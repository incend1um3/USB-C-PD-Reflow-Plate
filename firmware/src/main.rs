#![no_std]
#![no_main]

extern crate uom;

use core::panic;

use crate::pd::ucpd_task;
use crate::profiles::AlloyReflowProfile;
use crate::ui::draw_cooling_screen;
use defmt::{debug, info};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select, select_array};
use embassy_stm32::Peri;
use embassy_stm32::adc::{AdcChannel, AdcConfig, AnyAdcChannel, Rovsm, SampleTime, Trovs};
use embassy_stm32::timer::simple_pwm::SimplePwmChannel;
use embassy_stm32::{
    adc::Adc,
    bind_interrupts,
    exti::{self, ExtiInput},
    gpio::{OutputType, Pull},
    i2c::{self, I2c},
    interrupt, peripherals,
    time::khz,
    timer::simple_pwm::{PwmPin, SimplePwm},
};
use embassy_time::{Duration, Instant, Ticker, Timer, WithTimeout};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use pid::Pid;
use ssd1315::Ssd1315;
use ssd1315::interface::I2cDisplayInterface;

use {defmt_rtt as _, panic_probe as _};

mod pd;
mod profiles;
mod ui;

const HEATER_RESISTANCE: f32 = 2.68;

bind_interrupts!(
    struct Irqs {
        I2C1_ER => i2c::ErrorInterruptHandler<peripherals::I2C1>;
        I2C1_EV => i2c::EventInterruptHandler<peripherals::I2C1>;
        I2C2_ER => i2c::ErrorInterruptHandler<peripherals::I2C2>;
        I2C2_EV => i2c::EventInterruptHandler<peripherals::I2C2>;
        EXTI0 => exti::InterruptHandler<interrupt::typelevel::EXTI0>;
        EXTI1 => exti::InterruptHandler<interrupt::typelevel::EXTI1>;
        EXTI2 => exti::InterruptHandler<interrupt::typelevel::EXTI2>;
        EXTI4 => exti::InterruptHandler<interrupt::typelevel::EXTI4>;
        EXTI9_5 => exti::InterruptHandler<interrupt::typelevel::EXTI9_5>;
    }
);

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = embassy_stm32::Config::default();
    config.enable_ucpd1_dead_battery = true;

    let mut p = embassy_stm32::init(config);

    spawner.must_spawn(ucpd_task(p.UCPD1, p.PB6, p.PB4, p.DMA1_CH1, p.DMA1_CH2));

    let buzzer_pin = PwmPin::new(p.PA5, OutputType::PushPull);
    let mut buzzer_pwm_driver = SimplePwm::new(
        p.TIM2,
        Some(buzzer_pin),
        None,
        None,
        None,
        khz(10),
        Default::default(),
    );
    let mut buzzer = buzzer_pwm_driver.ch1();

    let plate_mosfet_pin = PwmPin::new(p.PA8, OutputType::PushPull);
    let mut plate_mosfet = SimplePwm::new(
        p.TIM1,
        Some(plate_mosfet_pin),
        None,
        None,
        None,
        khz(40),
        Default::default(),
    );
    let mut plate_mosfet = plate_mosfet.ch1();

    let mut adc_config = AdcConfig::default();
    // 0x02 = 8x oversampling
    // log2(8) = 3, 12 + 3 = 15 bits sampled
    adc_config.oversampling_ratio = Some(0x02);
    // 15 bits >> 1 = 14 bits returned
    // the extra samples act as a hardware mean filter
    adc_config.oversampling_shift = Some(1);
    adc_config.oversampling_mode = Some((Rovsm::RESUMED, Trovs::AUTOMATIC, true));

    let mut adc1 = Adc::new(p.ADC1, adc_config);
    let mut ntc_pin = p.PA0.degrade_adc();
    let mut vsys_pin = p.PA1.degrade_adc();

    let display_interface = I2cDisplayInterface::new_interface(I2c::new(
        p.I2C1,
        p.PA15,
        p.PB7,
        Irqs,
        p.DMA1_CH4,
        p.DMA1_CH5,
        i2c::Config::default(),
    ));

    let mut display = ssd1315::Ssd1315::new(display_interface);
    display.init_screen();

    let display_area = display.bounding_box();

    let mut pd_state_receiver = pd::STATE.receiver().unwrap();
    if let Ok(state) = pd_state_receiver
        .changed()
        .with_timeout(Duration::from_secs(1))
        .await
    {
        match state {
            pd::State::Good(_) => info!("USB PD negotiation complete"),
            pd::State::Error => {
                ui::draw_pd_error_screen(&mut display, &display_area);
                panic!("USB PD error");
            }
            _ => info!("USB PD negotation timed out. Assuming DC power."),
        }
    } else {
        info!("USB PD negotation timed out. Assuming DC power.")
    }

    let alloy_names: heapless::Vec<&str, 4> = profiles::PROFILES.iter().map(|p| p.alloy).collect();

    let _btn_up = ExtiInput::new(p.PA4, p.EXTI4, Pull::Down, Irqs);
    let _btn_down = ExtiInput::new(p.PB5, p.EXTI5, Pull::Down, Irqs);
    let mut btn_left = ExtiInput::new(p.PF0, p.EXTI0, Pull::Down, Irqs);
    let mut btn_right = ExtiInput::new(p.PF1, p.EXTI1, Pull::Down, Irqs);
    let mut btn_middle = ExtiInput::new(p.PA2, p.EXTI2, Pull::Down, Irqs);

    let mut selected_index = 0;

    buzzer.set_duty_cycle_percent(50);
    buzzer.enable();
    Timer::after_millis(500).await;
    buzzer.disable();

    loop {
        ui::draw_profiles(&mut display, &display_area, &alloy_names, selected_index);
        display.flush_screen();

        loop {
            match select_array([
                btn_left.wait_for_rising_edge(),
                btn_right.wait_for_rising_edge(),
                btn_middle.wait_for_rising_edge(),
            ])
            .await
            .1
            {
                // LEFT
                0 => {
                    selected_index =
                        (selected_index + profiles::PROFILES.len() - 1) % profiles::PROFILES.len();
                }
                // RIGHT
                1 => {
                    selected_index = (selected_index + 1) % profiles::PROFILES.len();
                }
                // MIDDLE
                2 => {
                    break;
                }
                _ => unreachable!(),
            }

            ui::draw_profiles(&mut display, &display_area, &alloy_names, selected_index);
            display.flush_screen();
        }

        let buttons_fut = btn_middle.wait_for_rising_edge();

        info!(
            "Starting reflow with profile: {}",
            profiles::PROFILES[selected_index].alloy
        );
        let reflow_fut = reflow(
            profiles::PROFILES[selected_index],
            &mut adc1,
            &mut ntc_pin,
            &mut vsys_pin,
            p.DMA2_CH1.reborrow(),
            &mut plate_mosfet,
            &mut display,
            display_area,
        );

        match select(reflow_fut, buttons_fut).await {
            Either::First(res) => {
                if res.is_err() {
                    for _ in 0..4 {
                        buzzer.enable();
                        Timer::after_millis(200).await;
                        buzzer.disable();
                        Timer::after_millis(100).await;
                    }
                } else {
                    buzzer.enable();
                    Timer::after_millis(500).await;
                    buzzer.disable();
                }
            }
            Either::Second(_) => {
                info!("Reflow cancelled by user");
                plate_mosfet.disable();
                buzzer.enable();
                Timer::after_millis(300).await;
                buzzer.disable();
            }
        };
    }
}

const R_PULLUP: f32 = 2200.0; // 2.2K
const R_NOMINAL: f32 = 100_000.0; // 100K @ 25°C
const B: f32 = 3950.0;
const T_NOMINAL: f32 = 298.15; // 25°C in Kelvin

fn adc_to_celsius(adc_raw: u16, adc_max: u16) -> f32 {
    let v_ratio = adc_raw as f32 / adc_max as f32;

    // NTC is on the low side, so:
    let r_ntc = R_PULLUP * v_ratio / (1.0 - v_ratio);

    let temp_k = 1.0 / (1.0 / T_NOMINAL + (1.0 / B) * libm::logf(r_ntc / R_NOMINAL));
    temp_k - 273.15
}

fn adc_to_voltage(adc_raw: u16, adc_max: u16) -> f32 {
    const VREF: f32 = 3.3;
    VREF * adc_raw as f32 / adc_max as f32
}

/// Computes the resistance of copper at a given temperature using a quadratic
/// polynomial model (IEC 60028 / IACS standard for annealed copper).
///
/// # Arguments
/// * `r20`  - Resistance at 20°C (in any consistent unit, e.g. Ohms)
/// * `temp` - Target temperature in °C (accurate from 0°C to 260°C)
///
/// # Returns
/// Resistance at `temp` in the same units as `r20`.
///
/// # Model
/// Uses the second-order polynomial:
///   R(T) = R₂₀ × (1 + α·ΔT + β·ΔT²)
///   where ΔT = T − 20°C
///
/// Coefficients (IEC 60028, annealed copper, 100% IACS):
///   α₀ = 4.041e-3 /°C  (linear coefficient at 0°C)
///   β  = −6.0e-7  /°C² (quadratic coefficient)
///   α₂₀ is derived by re-referencing α₀ and β to T=20°C.
fn copper_resistance(r20: f32, temp: f32) -> f32 {
    // IEC 60028 coefficients referenced to 0°C
    const ALPHA_0: f32 = 4.041e-3; // /°C
    const BETA: f32 = -6.0e-7; // /°C²

    // Re-reference to 20°C so the caller can pass R at 20°C directly.
    // R(T)   = R₀ · (1 + α₀·T  + β·T²)
    // R(20)  = R₀ · (1 + α₀·20 + β·400)
    // R(T)/R(20) = (1 + α₀·T + β·T²) / (1 + α₀·20 + β·400)
    let denom = 1.0 + ALPHA_0 * 20.0 + BETA * 400.0;
    let numer = 1.0 + ALPHA_0 * temp + BETA * temp * temp;

    r20 * (numer / denom)
}

fn maximum_pwm_duty_cycle_percentage(
    voltage: f32,
    maximum_current: f32,
    r20: f32,
    temp: f32,
) -> f32 {
    let r_t = copper_resistance(r20, temp);
    ((maximum_current * r_t / voltage) * 100.0).clamp(0.0, 100.0)
}

async fn read_ntc(
    mut dma: Peri<'_, peripherals::DMA2_CH1>,
    adc: &mut Adc<'static, peripherals::ADC1>,
    ntc_pin: &mut AnyAdcChannel<'_, peripherals::ADC1>,
) -> f32 {
    let mut readings = [0, 0, 0, 0, 0];
    adc.read(
        dma.reborrow(),
        [(&mut *ntc_pin, SampleTime::CYCLES247_5)].into_iter(),
        &mut readings,
    )
    .await;

    readings.sort_unstable();
    adc_to_celsius(readings[2], 2u16.pow(14) - 1)
}

async fn reflow<I: display_interface::WriteOnlyDataCommand>(
    profile: &'static AlloyReflowProfile,
    adc: &mut Adc<'static, peripherals::ADC1>,
    ntc_pin: &mut AnyAdcChannel<'_, peripherals::ADC1>,
    vsys_pin: &mut AnyAdcChannel<'_, peripherals::ADC1>,
    mut dma: Peri<'_, peripherals::DMA2_CH1>,
    plate_pwm: &mut SimplePwmChannel<'_, peripherals::TIM1>,
    display: &mut Ssd1315<I>,
    display_area: Rectangle,
) -> Result<(), ()> {
    plate_pwm.set_duty_cycle(0);
    plate_pwm.enable();

    let pd_state = pd::STATE.try_get().unwrap();

    // USB-C PD chargers typically have a maximum current limit of 5A.
    // If we're running on a plain DC supply, assume we're allowed a maximum of 10A.
    let maximum_current = match pd_state {
        pd::State::Good(ref limits) => limits.max_current - 0.1,
        pd::State::NotAttached => 9.9,
        pd::State::Error => return Err(()),
    };

    let voltage = if let pd::State::Good(limits) = pd_state {
        limits.voltage
    } else {
        let reading = adc.blocking_read(vsys_pin, SampleTime::CYCLES640_5);
        let voltage = adc_to_voltage(reading, 2u16.pow(12) - 1);

        const R_UP: f32 = 47_000f32;
        const R_DOWN: f32 = 4_700f32;

        voltage * ((R_UP + R_DOWN) / R_DOWN)
    };

    let initial_temp = read_ntc(dma.reborrow(), adc, ntc_pin).await;

    let beginning = Instant::now();

    let mut pid: Pid<f32> = Pid::new(0.0, 100.0);
    pid.p(10.0, 100.0);
    pid.i(0.5, 25.0);

    let mut ticker = Ticker::every(Duration::from_hz(10));
    let mut display_refresh_counter = 0u8;
    let mut previous_temp = None;
    loop {
        let raw_temp = read_ntc(dma.reborrow(), adc, ntc_pin).await;
        // EMA Filter
        let temp = if let Some(t) = previous_temp {
            0.3 * raw_temp + 0.7 * t
        } else {
            previous_temp = Some(raw_temp);
            raw_temp
        };

        debug!("Temperature: {}°C", temp as i16);

        if let Some(reflow_status) =
            profile.get_status(beginning.elapsed().as_secs() as u16, initial_temp)
        {
            debug!(
                "Time left in phase: {} seconds",
                reflow_status.time_left_in_phase
            );

            pid.output_limit = maximum_pwm_duty_cycle_percentage(
                voltage,
                maximum_current,
                HEATER_RESISTANCE,
                temp,
            );
            pid.setpoint(reflow_status.target_temp);

            let duty_cycle = libm::roundf(pid.next_control_output(temp).output.max(0.0)) as u8;
            plate_pwm.set_duty_cycle_percent(duty_cycle);

            if display_refresh_counter == 0 {
                ui::draw_progress(
                    display,
                    display_area,
                    beginning.elapsed(),
                    &reflow_status,
                    temp,
                )
                .unwrap();
                display.flush_screen();
            }
            display_refresh_counter = (display_refresh_counter + 1) % 5;

            ticker.next().await;
        } else {
            info!("Completed all heating phases");
            break;
        }
    }

    plate_pwm.disable();
    info!("Starting cooling phase");

    let mut ticker = Ticker::every(Duration::from_hz(2));
    loop {
        let temp = read_ntc(dma.reborrow(), adc, ntc_pin).await;
        draw_cooling_screen(display, &display_area, temp).unwrap();

        if temp < 45.0 {
            info!("Cooling done");
            break;
        }

        ticker.next().await;
    }

    Ok(())
}

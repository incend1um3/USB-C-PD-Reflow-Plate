use defmt::{debug, error, info, warn};
use embassy_futures::select::select;
use embassy_stm32::ucpd::{CcPull, CcSel};
use embassy_stm32::{Peri, gpio};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::watch::Watch;
use embassy_time::{Duration, Timer, with_timeout};
use uom::si::electric_current::milliampere;
use uom::si::electric_potential::{millivolt, volt};
use usbpd::protocol_layer::message::data::request::{self, CurrentRequest};
use usbpd::protocol_layer::message::data::source_capabilities::{
    Augmented, PowerDataObject, SourceCapabilities,
};
use usbpd::sink::device_policy_manager::DevicePolicyManager;
use usbpd::sink::policy_engine::Sink;
use usbpd::timers::Timer as SinkTimer;
use usbpd::units::ElectricPotential;
use usbpd_traits::Driver as SinkDriver;

use embassy_stm32::{
    bind_interrupts, peripherals,
    ucpd::{self, CcPhy, CcVState, PdPhy, Ucpd},
};

const MAX_PD_VOLTAGE: u32 = 22;

bind_interrupts!(struct Irqs {
    UCPD1 => ucpd::InterruptHandler<peripherals::UCPD1>;

});

#[derive(Clone, PartialEq)]
pub struct NegotiatedPdPowerLimits {
    pub voltage: f32,
    pub max_current: f32,
}

#[derive(Clone, PartialEq)]
pub enum State {
    NotAttached,
    Good(NegotiatedPdPowerLimits),
    Error,
}

pub static STATE: Watch<ThreadModeRawMutex, State, 1> = Watch::new_with(State::NotAttached);

#[derive(Debug, defmt::Format)]
enum CableOrientation {
    Normal,
    Flipped,
    DebugAccessoryMode,
}

struct UcpdSinkDriver<'d> {
    /// The UCPD PD phy instance.
    pd_phy: PdPhy<'d, peripherals::UCPD1>,
}

impl<'d> UcpdSinkDriver<'d> {
    fn new(pd_phy: PdPhy<'d, peripherals::UCPD1>) -> Self {
        Self { pd_phy }
    }
}

impl SinkDriver for UcpdSinkDriver<'_> {
    async fn wait_for_vbus(&mut self) {
        // The sink policy engine is only running when attached. Therefore VBus is present.
    }

    async fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, usbpd_traits::DriverRxError> {
        self.pd_phy.receive(buffer).await.map_err(|err| match err {
            ucpd::RxError::Crc | ucpd::RxError::Overrun => usbpd_traits::DriverRxError::Discarded,
            ucpd::RxError::HardReset => usbpd_traits::DriverRxError::HardReset,
        })
    }

    async fn transmit(&mut self, data: &[u8]) -> Result<(), usbpd_traits::DriverTxError> {
        self.pd_phy.transmit(data).await.map_err(|err| match err {
            ucpd::TxError::Discarded => usbpd_traits::DriverTxError::Discarded,
            ucpd::TxError::HardReset => usbpd_traits::DriverTxError::HardReset,
        })
    }

    async fn transmit_hard_reset(&mut self) -> Result<(), usbpd_traits::DriverTxError> {
        self.pd_phy
            .transmit_hardreset()
            .await
            .map_err(|err| match err {
                ucpd::TxError::Discarded => usbpd_traits::DriverTxError::Discarded,
                ucpd::TxError::HardReset => usbpd_traits::DriverTxError::HardReset,
            })
    }
}

#[derive(Default)]
struct Device {}

impl DevicePolicyManager for Device {
    async fn request(&mut self, source_capabilities: &SourceCapabilities) -> request::PowerSource {
        info!(
            "Found USB-C PD source capabilities: {}",
            source_capabilities
        );

        let selected_epr_supply = source_capabilities
            .epr_pdos()
            .filter_map(|(_, pdo)| match pdo {
                PowerDataObject::Augmented(Augmented::Epr(epr)) => {
                    if epr.min_voltage().get::<volt>() <= MAX_PD_VOLTAGE {
                        Some(epr)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .max_by_key(|epr| epr.pd_power());

        let selected_spr_supply = source_capabilities
            .spr_pdos()
            .filter_map(|(_, pdo)| match pdo {
                PowerDataObject::Augmented(Augmented::Spr(spr)) => {
                    if spr.min_voltage().get::<volt>() <= MAX_PD_VOLTAGE {
                        Some(spr)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .max_by_key(|spr| spr.max_voltage() * spr.max_current());

        let best_fixed = source_capabilities
            .pdos()
            .iter()
            .filter_map(|pdo| match pdo {
                PowerDataObject::FixedSupply(supply) => Some(supply),
                _ => None,
            })
            .filter(|s| {
                s.voltage().get::<volt>() >= 12 && s.voltage().get::<volt>() <= MAX_PD_VOLTAGE
            })
            .max_by_key(|s| s.voltage() * s.max_current());

        if let Some(epr_supply) = selected_epr_supply
            && epr_supply.max_voltage()
                >= selected_spr_supply
                    .map(|s| s.max_voltage())
                    .unwrap_or(ElectricPotential::new::<volt>(0))
            && epr_supply.max_voltage()
                >= best_fixed
                    .map(|s| s.voltage())
                    .unwrap_or(ElectricPotential::new::<volt>(0))
        {
            let voltage = min_voltage(
                epr_supply.max_voltage(),
                ElectricPotential::new::<volt>(MAX_PD_VOLTAGE),
            );
            if let Ok(r) = request::PowerSource::new_epr_avs(
                CurrentRequest::Highest,
                voltage,
                source_capabilities,
            ) {
                STATE.sender().send(State::Good(NegotiatedPdPowerLimits {
                    voltage: voltage.get::<millivolt>() as f32 / 1000.0,
                    max_current: (epr_supply.pd_power() / voltage).get::<milliampere>() as f32
                        / 1000.0,
                }));

                info!("Power source selected: {}", epr_supply);
                return r;
            } else {
                error!("Failed to negotiate an EPR AVS power source");
            }
        }

        if let Some(spr_supply) = selected_spr_supply
            && spr_supply.max_voltage()
                >= best_fixed
                    .map(|s| s.voltage())
                    .unwrap_or(ElectricPotential::new::<volt>(0))
        {
            let voltage = min_voltage(
                spr_supply.max_voltage(),
                ElectricPotential::new::<volt>(MAX_PD_VOLTAGE),
            );
            if let Ok(r) =
                request::PowerSource::new_pps(CurrentRequest::Highest, voltage, source_capabilities)
            {
                STATE.sender().send(State::Good(NegotiatedPdPowerLimits {
                    voltage: voltage.get::<millivolt>() as f32 / 1000.0,
                    max_current: spr_supply.max_current().get::<milliampere>() as f32 / 1000.0,
                }));

                info!("Power source selected: {}", spr_supply);
                return r;
            } else {
                error!("Failed to negotiate a PPS power source");
            }
        }

        match best_fixed {
            Some(fixed_supply) => {
                if let Ok(r) = request::PowerSource::new_fixed(
                    request::CurrentRequest::Highest,
                    request::VoltageRequest::Specific(fixed_supply.voltage()),
                    source_capabilities,
                ) {
                    STATE.sender().send(State::Good(NegotiatedPdPowerLimits {
                        voltage: fixed_supply.voltage().get::<millivolt>() as f32 / 1000.0,
                        max_current: fixed_supply.max_current().get::<milliampere>() as f32
                            / 1000.0,
                    }));

                    info!("Power source selected: {}", fixed_supply);
                    r
                } else {
                    error!("Failed to negotiate a fixed power source");

                    STATE.sender().send(State::Error);

                    request::PowerSource::new_fixed(
                        request::CurrentRequest::Highest,
                        request::VoltageRequest::Safe5V,
                        source_capabilities,
                    )
                    .expect("Failed to select a suitable PD source")
                }
            }
            None => panic!("Failed to select a suitable PD source"),
        }
    }
}

async fn wait_attached<T: ucpd::Instance>(cc_phy: &mut CcPhy<'_, T>) -> CableOrientation {
    loop {
        let (cc1, cc2) = cc_phy.vstate();
        if cc1 == CcVState::LOWEST && cc2 == CcVState::LOWEST {
            // Detached, wait until attached by monitoring the CC lines.
            cc_phy.wait_for_vstate_change().await;
            continue;
        }

        // Attached, wait for CC lines to be stable for tCCDebounce (100..200ms).
        if with_timeout(Duration::from_millis(100), cc_phy.wait_for_vstate_change())
            .await
            .is_ok()
        {
            // State has changed, restart detection procedure.
            continue;
        };

        // State was stable for the complete debounce period, check orientation.
        return match (cc1, cc2) {
            (_, CcVState::LOWEST) => CableOrientation::Normal, // CC1 connected
            (CcVState::LOWEST, _) => CableOrientation::Flipped, // CC2 connected
            _ => CableOrientation::DebugAccessoryMode,         // Both connected (special cable)
        };
    }
}

async fn wait_detached<T: ucpd::Instance>(cc_phy: &mut CcPhy<'_, T>) {
    loop {
        let (cc1, cc2) = cc_phy.vstate();
        if cc1 == CcVState::LOWEST && cc2 == CcVState::LOWEST {
            return;
        }
        cc_phy.wait_for_vstate_change().await;
    }
}

struct EmbassySinkTimer {}

impl SinkTimer for EmbassySinkTimer {
    async fn after_millis(milliseconds: u64) {
        Timer::after_millis(milliseconds).await
    }
}

#[embassy_executor::task]
pub async fn ucpd_task(
    mut ucpd: Peri<'static, peripherals::UCPD1>,
    mut cc1: Peri<'static, peripherals::PB6>,
    mut cc2: Peri<'static, peripherals::PB4>,
    mut dma1: Peri<'static, peripherals::DMA1_CH1>,
    mut dma2: Peri<'static, peripherals::DMA1_CH2>,
    pd_flt: gpio::Input<'static>,
) {
    loop {
        let mut ucpd = Ucpd::new(
            ucpd.reborrow(),
            Irqs,
            cc1.reborrow(),
            cc2.reborrow(),
            ucpd::Config::default(),
        );
        ucpd.cc_phy().set_pull(CcPull::Sink);

        let cable_orientation = wait_attached(ucpd.cc_phy()).await;

        let cc_sel = match cable_orientation {
            CableOrientation::Normal => {
                info!("Starting PD communication on CC1 pin");
                CcSel::CC1
            }
            CableOrientation::Flipped => {
                info!("Starting PD communication on CC2 pin");
                CcSel::CC2
            }
            CableOrientation::DebugAccessoryMode => {
                debug!("PD_FLT: {}", if pd_flt.is_high() { "HIGH" } else { "LOW" });
                panic!("No PD communication in DAM");
            }
        };

        let (mut cc_phy, pd_phy) =
            ucpd.split_pd_phy(dma1.reborrow(), dma2.reborrow(), crate::Irqs, cc_sel);

        let driver = UcpdSinkDriver::new(pd_phy);
        let mut sink: Sink<UcpdSinkDriver<'_>, EmbassySinkTimer, _> =
            Sink::new(driver, Device::default());

        match select(sink.run(), wait_detached(&mut cc_phy)).await {
            embassy_futures::select::Either::First(result) => {
                debug!("PD_FLT: {}", if pd_flt.is_high() { "HIGH" } else { "LOW" });
                warn!("Sink loop broken with result: {}", result)
            }
            embassy_futures::select::Either::Second(_) => {
                debug!("PD_FLT: {}", if pd_flt.is_high() { "HIGH" } else { "LOW" });
                info!("Detached");
                continue;
            }
        }
    }
}

fn min_voltage(v1: ElectricPotential, v2: ElectricPotential) -> ElectricPotential {
    if v1 < v2 { v1 } else { v2 }
}

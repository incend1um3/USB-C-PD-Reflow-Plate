use core::fmt::Write;
use core::str::FromStr;
use defmt::debug;
use embassy_time::Duration;
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_7X13, FONT_8X13, FONT_9X15};
use embedded_graphics::mono_font::iso_8859_7::FONT_4X6;
use embedded_graphics::mono_font::iso_8859_9::FONT_5X7;
use embedded_graphics::mono_font::jis_x0201::FONT_10X20;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Alignment::Center;
use embedded_graphics::text::Baseline;
use embedded_graphics::{
    mono_font::MonoTextStyle, pixelcolor::BinaryColor, prelude::*, text::Text,
};
use embedded_layout::align::{Align, Alignment, horizontal, vertical};
use embedded_layout::layout::linear::{LinearLayout, spacing};
use embedded_layout::prelude::Chain;
use heapless::String;

use crate::pd;
use crate::profiles::ReflowStatus;

enum TemperatureFormatting {
    Target,
    Actual,
    None,
}

// Avoid core::fmt::float because it takes up 7+ KiB in flash.
fn format_temp(temp: f32, target: TemperatureFormatting) -> heapless::String<20> {
    let t = libm::roundf(temp * 10.0) as i32;
    let whole = t / 10;
    let frac = (t % 10).unsigned_abs();
    let mut buf = heapless::String::new();
    match target {
        TemperatureFormatting::Target => write!(buf, "Tar: {}.{}°C", whole, frac).unwrap(),
        TemperatureFormatting::Actual => write!(buf, "Act: {}.{}°C", whole, frac).unwrap(),
        TemperatureFormatting::None => write!(buf, "{}.{}°C", whole, frac).unwrap(),
    }
    buf
}

// Avoid core::fmt::float because it takes up 7+ KiB in flash.
fn format_voltage(temp: f32) -> heapless::String<10> {
    let t = libm::roundf(temp * 10.0) as i32;
    let whole = t / 10;
    let frac = (t % 10).unsigned_abs();
    let mut buf = heapless::String::new();
    write!(buf, "VCC: {}.{}V", whole, frac).unwrap();
    buf
}

fn arrange_rows(
    max_width: u32,
    strings: &[&str],
    char_size: embedded_graphics::geometry::Size,
) -> heapless::Vec<u8, 4> {
    let mut rows = heapless::Vec::new();

    const SPACING: u32 = 2;
    let mut column = 0;
    let mut row_width = 0;
    let mut items_in_row = 0;
    for s in strings {
        let w = s.len() as u32 * char_size.width + if column > 0 { SPACING } else { 0 };
        if row_width + w > max_width {
            rows.push(items_in_row).unwrap();

            row_width = 0;
            items_in_row = 0;
            column += 1;
        }

        row_width += w;
        items_in_row += 1;
    }

    rows.push(items_in_row).unwrap();

    rows
}

pub fn draw_profiles<D>(
    display: &mut D,
    display_area: &Rectangle,
    items: &[&str],
    selected: usize,
    pd_state: &pd::State,
    vcc_voltage: f32,
) where
    D: DrawTarget<Color = BinaryColor>,
    D::Error: core::fmt::Debug,
{
    display.clear(BinaryColor::Off).unwrap();

    Text::new(
        "Select Profile",
        Point::zero(),
        MonoTextStyle::new(&FONT_8X13, BinaryColor::On),
    )
    .align_to(display_area, horizontal::Center, vertical::Top)
    .draw(display)
    .unwrap();

    const FONT: &embedded_graphics::mono_font::MonoFont = &FONT_6X10;
    const Y_SPACING: u32 = 8;
    const X_MARGIN: u32 = 2;

    let style_normal = MonoTextStyle::new(&FONT, BinaryColor::On);
    let fill_selected = PrimitiveStyle::with_stroke(BinaryColor::On, 1);

    let rows = arrange_rows(display_area.size.width, items, FONT.character_size);

    let mut elem_start = 0;
    let mut y = 30;
    for num_elements in rows {
        let row_width: u32 = items
            .iter()
            .skip(elem_start)
            .map(|s| s.len() as u32 * FONT.character_size.width)
            .take(num_elements as usize)
            .sum();
        let row_width = row_width + X_MARGIN * 4;

        let mut x = (display_area.size.width - row_width) / 2;
        for e in 0..num_elements {
            let elem_width =
                items[elem_start + e as usize].len() as u32 * FONT.character_size.width;
            let rect = Rectangle::new(
                Point::new(x as i32 - 2, y as i32 - 2),
                Size::new(elem_width + 4, FONT.character_size.height + 4),
            );

            if selected == elem_start + e as usize {
                rect.into_styled(fill_selected).draw(display).unwrap();
                Text::with_baseline(
                    items[elem_start + e as usize],
                    Point::new(x as i32, y as i32),
                    style_normal,
                    Baseline::Top,
                )
                .draw(display)
                .unwrap();
            } else {
                Text::with_baseline(
                    items[elem_start + e as usize],
                    Point::new(x as i32, y as i32),
                    style_normal,
                    Baseline::Top,
                )
                .draw(display)
                .unwrap();
            }

            x += elem_width + X_MARGIN * 4;
        }

        y = FONT.character_size.height + Y_SPACING;
        elem_start += num_elements as usize;
    }

    let text = match pd_state {
        pd::State::Good(_) => "PD State: Good",
        pd::State::NotAttached => "PD State: N/A",
        pd::State::Error => "PD State: Error",
    };

    let voltage = format_voltage(vcc_voltage);

    let text_style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    LinearLayout::horizontal(
        Chain::new(Text::new(text, Point::zero(), text_style)).append(Text::new(
            &voltage,
            Point::zero(),
            text_style,
        )),
    )
    .with_spacing(spacing::FixedMargin(4))
    .arrange()
    .align_to(display_area, horizontal::Center, vertical::Bottom)
    .draw(display)
    .unwrap();
}

fn format_secs(total_secs: u64) -> String<8> {
    let m = total_secs / 60;
    let s = total_secs % 60;

    let mut buf = String::new();
    write!(buf, "{:02}:{:02}", m, s).unwrap();
    buf
}

fn format_duty_cycle(percent: u8) -> String<20> {
    let mut buf = String::new();
    write!(buf, "Duty Cycle: {}%", percent).unwrap();
    buf
}

pub fn draw_progress<D>(
    display: &mut D,
    display_area: Rectangle,
    elapsed: Duration,
    status: &ReflowStatus,
    temp: f32,
    duty_cycle_percentage: u8,
    vsys_voltage: f32,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
    D::Error: core::fmt::Debug,
{
    display.clear(BinaryColor::Off)?;

    Text::new(
        status.phase_name.into(),
        Point::zero(),
        MonoTextStyle::new(&FONT_8X13, BinaryColor::On),
    )
    .align_to(&display_area, horizontal::Center, vertical::Top)
    .draw(display)?;

    let area = Rectangle::new(
        Point::new(0, FONT_8X13.character_size.height as i32 + 5),
        Size::new(
            128,
            display_area.size.height - FONT_8X13.character_size.height - 5,
        ),
    );

    let row2_style = MonoTextStyle::new(&FONT_5X7, BinaryColor::On);

    let mut elapsed_text: String<20> = String::from_str("Elap: ").unwrap();
    elapsed_text
        .push_str(format_secs(elapsed.as_secs()).as_str())
        .unwrap();

    let mut remaining_text: String<20> = String::from_str("Rem: ").unwrap();
    remaining_text
        .push_str(&format_secs(status.total_time_left as u64))
        .unwrap();

    LinearLayout::vertical(
        Chain::new(
            LinearLayout::horizontal(
                Chain::new(Text::new(&elapsed_text, Point::zero(), row2_style)).append(Text::new(
                    &remaining_text,
                    Point::zero(),
                    row2_style,
                )),
            )
            .with_spacing(spacing::DistributeFill(128))
            .arrange(),
        )
        .append(
            LinearLayout::horizontal(
                Chain::new(Text::new(
                    &format_temp(temp, TemperatureFormatting::Actual),
                    Point::zero(),
                    row2_style,
                ))
                .append(Text::new(
                    &format_temp(status.target_temp, TemperatureFormatting::Target),
                    Point::zero(),
                    row2_style,
                )),
            )
            .with_spacing(spacing::DistributeFill(128))
            .arrange(),
        )
        .append(
            LinearLayout::horizontal(
                Chain::new(Text::new(
                    &format_duty_cycle(duty_cycle_percentage),
                    Point::zero(),
                    row2_style,
                ))
                .append(Text::new(
                    &format_voltage(vsys_voltage),
                    Point::zero(),
                    row2_style,
                )),
            )
            .with_spacing(spacing::DistributeFill(128))
            .arrange(),
        ),
    )
    .align_to(&area, horizontal::Center, vertical::Top)
    .with_spacing(spacing::FixedMargin(4))
    .arrange()
    .draw(display)?;

    const BAR_HEIGHT: i32 = 8;
    const BORDER_MARGIN: i32 = 1;
    const BORDER_WIDTH: i32 = 1;
    let border_start = Point::new(
        0,
        display_area.size.height as i32 - BAR_HEIGHT - (BORDER_MARGIN + BORDER_WIDTH) * 2,
    );
    let border_size = Size::new(
        display_area.size.width,
        (BAR_HEIGHT + (BORDER_MARGIN + BORDER_WIDTH) * 2) as u32,
    );
    let bar_start = Point::new(
        BORDER_WIDTH + BORDER_MARGIN,
        display_area.size.height as i32 - BORDER_WIDTH - BORDER_MARGIN - BAR_HEIGHT,
    );
    let progress =
        (elapsed.as_secs()) as f32 / (elapsed.as_secs() as u16 + status.total_time_left) as f32;
    let bar_size = Size::new(
        ((display_area.size.width - (BORDER_WIDTH + BORDER_MARGIN) as u32 * 2) as f32 * progress)
            as u32,
        BAR_HEIGHT as u32,
    );

    // this crashes for some reason
    // LinearLayout::vertical(
    //     Chain::new(Text::new(&remaining_text, Point::zero(), row2_style)).append(
    //         Rectangle::new(Point::zero(), border_size).into_styled(PrimitiveStyle::with_stroke(
    //             BinaryColor::On,
    //             BORDER_WIDTH as u32,
    //         )),
    //     ),
    // )
    // .align_to(&display_area, horizontal::Center, vertical::Bottom)
    // .with_spacing(spacing::FixedMargin(4))
    // .arrange()
    // .draw(display)?;

    Rectangle::new(border_start, border_size)
        .into_styled(PrimitiveStyle::with_stroke(
            BinaryColor::On,
            BORDER_WIDTH as u32,
        ))
        .draw(display)?;

    Rectangle::new(bar_start, bar_size)
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;

    Ok(())
}

pub fn draw_cooling_screen<D>(
    display: &mut D,
    display_area: &Rectangle,
    temp: f32,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
    D::Error: core::fmt::Debug,
{
    display.clear(BinaryColor::Off)?;
    LinearLayout::vertical(
        Chain::new(Text::new(
            "Cooling",
            Point::zero(),
            MonoTextStyle::new(&FONT_10X20, BinaryColor::On),
        ))
        .append(Text::new(
            &format_temp(temp, TemperatureFormatting::None),
            Point::zero(),
            MonoTextStyle::new(&FONT_8X13, BinaryColor::On),
        )),
    )
    .align_to(display_area, horizontal::Center, vertical::Center)
    .with_spacing(spacing::FixedMargin(8))
    .arrange()
    .draw(display)
}

pub fn draw_pd_error_screen<D>(display: &mut D, display_area: &Rectangle) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
    D::Error: core::fmt::Debug,
{
    display.clear(BinaryColor::Off)?;
    Text::new(
        "PD Negotiation\nFailed",
        Point::zero(),
        MonoTextStyle::new(&FONT_9X15, BinaryColor::On),
    )
    .align_to(display_area, horizontal::Center, vertical::Center)
    .draw(display)?;

    Ok(())
}

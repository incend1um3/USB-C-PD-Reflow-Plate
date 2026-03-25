use core::fmt::Write;
use core::str::FromStr;
use embassy_time::Duration;
use embedded_graphics::mono_font::ascii::FONT_8X13;
use embedded_graphics::mono_font::jis_x0201::FONT_10X20;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::{
    mono_font::MonoTextStyle, pixelcolor::BinaryColor, prelude::*, text::Text,
};
use embedded_layout::align::{Align, horizontal, vertical};
use embedded_layout::layout::linear::{LinearLayout, spacing};
use embedded_layout::prelude::Chain;
use heapless::String;

use crate::profiles::ReflowStatus;

// Avoid core::fmt::float because it takes up 7+ KiB in flash.
fn format_temp(temp: f32) -> heapless::String<10> {
    let t = libm::roundf(temp * 10.0) as i32;
    let whole = t / 10;
    let frac = (t % 10).unsigned_abs();
    let mut buf = heapless::String::new();
    write!(buf, "{}.{}°C", whole, frac).unwrap();
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

pub fn draw_profiles<D>(display: &mut D, display_area: &Rectangle, items: &[&str], selected: usize)
where
    D: DrawTarget<Color = BinaryColor>,
    D::Error: core::fmt::Debug,
{
    display.clear(BinaryColor::Off).unwrap();

    Text::new(
        "Select Profile",
        Point::zero(),
        MonoTextStyle::new(&FONT_10X20, BinaryColor::On),
    )
    .align_to(display_area, horizontal::Center, vertical::Top)
    .draw(display)
    .unwrap();

    const FONT: &embedded_graphics::mono_font::MonoFont = &FONT_8X13;
    const Y_SPACING: u32 = 8;

    let style_normal = MonoTextStyle::new(&FONT, BinaryColor::On);
    let style_inverted = MonoTextStyle::new(&FONT, BinaryColor::Off);
    let fill_selected = PrimitiveStyle::with_fill(BinaryColor::On);
    let fill_unselected = PrimitiveStyle::with_stroke(BinaryColor::On, 1);

    let rows = arrange_rows(display_area.size.width, items, FONT.character_size);

    let mut elem_start = 0;
    let mut y = 30;
    for num_elements in rows {
        let row_width: u32 = items
            .iter()
            .skip(elem_start)
            .map(|s| s.len() as u32 * FONT.character_size.width)
            .sum();

        let mut x = (display_area.size.width - row_width) / 2;
        for e in 0..num_elements {
            let elem_width =
                items[elem_start + e as usize].len() as u32 * FONT.character_size.width;
            let rect = Rectangle::new(
                Point::new(x as i32, y as i32),
                Size::new(elem_width, FONT.character_size.height),
            );

            if selected == elem_start + e as usize {
                rect.into_styled(fill_selected).draw(display).unwrap();
                Text::new(
                    items[elem_start + e as usize],
                    Point::new(x as i32, y as i32),
                    style_inverted,
                )
                .draw(display)
                .unwrap();
            } else {
                rect.into_styled(fill_unselected).draw(display).unwrap();
                Text::new(
                    items[elem_start + e as usize],
                    Point::new(x as i32, y as i32),
                    style_normal,
                )
                .draw(display)
                .unwrap();
            }

            x += elem_width;
        }

        y = FONT.character_size.height + Y_SPACING;
        elem_start += num_elements as usize;
    }
}

fn format_secs(total_secs: u64) -> String<6> {
    let m = total_secs / 60;
    let s = total_secs % 60;

    let mut buf = String::new();
    write!(buf, "{:02}:{:02}", m, s).unwrap();
    buf
}

pub fn draw_progress<D>(
    display: &mut D,
    display_area: Rectangle,
    elapsed: Duration,
    status: &ReflowStatus,
    temp: f32,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
    D::Error: core::fmt::Debug,
{
    display.clear(BinaryColor::Off)?;

    let row2_style = MonoTextStyle::new(&FONT_8X13, BinaryColor::On);

    let mut elapsed_text: String<20> = String::from_str("Elapsed: ").unwrap();
    elapsed_text
        .push_str(format_secs(elapsed.as_secs()).as_str())
        .unwrap();

    let mut remaining_text: String<20> = String::from_str("Remaining: ").unwrap();
    remaining_text
        .push_str(&format_secs(status.total_time_left as u64))
        .unwrap();

    LinearLayout::vertical(
        Chain::new(Text::new(
            status.phase_name.into(),
            Point::zero(),
            MonoTextStyle::new(&FONT_10X20, BinaryColor::On),
        ))
        .append(
            LinearLayout::horizontal(
                Chain::new(Text::new(&elapsed_text, Point::zero(), row2_style)).append(Text::new(
                    &format_temp(temp),
                    Point::zero(),
                    row2_style,
                )),
            )
            .arrange(),
        ),
    )
    .align_to(&display_area, horizontal::Center, vertical::Center)
    .with_spacing(spacing::FixedMargin(8))
    .arrange()
    .draw(display)?;

    const BAR_HEIGHT: i32 = 16;
    const BORDER_MARGIN: i32 = 2;
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
    let progress = elapsed.as_secs() as f32 / status.total_time_left as f32;
    let bar_size = Size::new(
        ((display_area.size.width - (BORDER_WIDTH + BORDER_MARGIN) as u32 * 2) as f32 * progress)
            as u32,
        BAR_HEIGHT as u32,
    );

    LinearLayout::vertical(
        Chain::new(Text::new(&remaining_text, Point::zero(), row2_style)).append(
            Rectangle::new(border_start, border_size).into_styled(PrimitiveStyle::with_stroke(
                BinaryColor::On,
                BORDER_WIDTH as u32,
            )),
        ),
    )
    .align_to(&display_area, horizontal::Center, vertical::Bottom)
    .with_spacing(spacing::FixedMargin(4))
    .arrange()
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
            &format_temp(temp),
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
        "PD Negotiation Error",
        Point::zero(),
        MonoTextStyle::new(&FONT_10X20, BinaryColor::On),
    )
    .align_to(display_area, horizontal::Center, vertical::Center)
    .draw(display)?;

    Ok(())
}

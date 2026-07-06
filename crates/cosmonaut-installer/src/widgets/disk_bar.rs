//! The disk map — the installer's signature visual. A proportional,
//! role-colored strip of the whole device with a dot-chip legend.
//!
//! Not a custom iced widget: a `row` of styled containers with
//! `Length::FillPortion` weights, so it inherits theming for free.
//! Segments are always scaled against the *device* size; any space the
//! segments don't cover (unprivileged probes can't see gaps) renders as
//! an implicit "unallocated" filler, so a 14 GiB partition on a 116 GiB
//! disk reads as the sliver it is.

use cosmic::iced::{Alignment, Border, Length};
use cosmic::widget::{column, container, row, text};
use cosmic::{theme, Element};

use cosmonaut_engine::probe::{human_size, DiskInfo};

use crate::app::Message;

/// What a bar segment will be (or is), which drives its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegRole {
    /// EFI system partition (new or kept).
    Esp,
    /// The new root filesystem — the reason we're here; brightest color.
    Root,
    /// An existing partition left untouched. Quiet by design.
    Existing,
    /// Unallocated space.
    Free,
}

#[derive(Debug, Clone)]
pub struct BarSegment {
    pub label: String,
    pub size_bytes: u64,
    pub role: SegRole,
}

/// Segments describing the disk as it is now: partitions (by on-disk
/// order) interleaved with known free-space gaps. The bar itself adds
/// the unknown remainder.
pub fn segments_current(disk: &DiskInfo) -> Vec<BarSegment> {
    #[derive(Debug)]
    enum Item {
        Part(usize),
        Gap(usize),
    }
    // Partitions are start-sorted by the probe; merge with gaps by start.
    let mut items: Vec<(u64, Item)> = disk
        .partitions
        .iter()
        .enumerate()
        .map(|(i, p)| (p.start_bytes, Item::Part(i)))
        .chain(
            disk.gaps
                .iter()
                .enumerate()
                .map(|(i, g)| (g.start_bytes, Item::Gap(i))),
        )
        .collect();
    items.sort_by_key(|(start, _)| *start);

    items
        .into_iter()
        .map(|(_, item)| match item {
            Item::Part(i) => {
                let p = &disk.partitions[i];
                let name = p
                    .detected_os
                    .as_ref()
                    .map(|os| os.pretty_name.clone())
                    .or_else(|| p.label.clone())
                    .or_else(|| p.fstype.clone())
                    .unwrap_or_else(|| format!("partition {}", p.number));
                BarSegment {
                    label: name,
                    size_bytes: p.size_bytes,
                    role: SegRole::Existing,
                }
            }
            Item::Gap(i) => BarSegment {
                label: "free space".into(),
                size_bytes: disk.gaps[i].size_bytes,
                role: SegRole::Free,
            },
        })
        .collect()
}

/// Segments for the erase-whole-disk layout the engine will create:
/// mirrors `cosmonaut_engine::partition`'s ESP 512 MiB + boot 1 GiB +
/// root (rest) script.
pub fn segments_erase(disk: &DiskInfo) -> Vec<BarSegment> {
    const ESP: u64 = 512 * 1024 * 1024;
    const BOOT: u64 = 1024 * 1024 * 1024;
    let root = disk.size_bytes.saturating_sub(ESP + BOOT);
    vec![
        BarSegment {
            label: "EFI".into(),
            size_bytes: ESP,
            role: SegRole::Esp,
        },
        BarSegment {
            label: "boot".into(),
            size_bytes: BOOT,
            role: SegRole::Existing,
        },
        BarSegment {
            label: "new system".into(),
            size_bytes: root,
            role: SegRole::Root,
        },
    ]
}

fn role_color(role: SegRole, theme: &cosmic::Theme) -> cosmic::iced::Color {
    let palette = &theme.cosmic().palette;
    let c = match role {
        SegRole::Esp => palette.accent_orange,
        SegRole::Root => palette.accent_blue,
        SegRole::Existing => palette.neutral_6,
        SegRole::Free => palette.neutral_3,
    };
    cosmic::iced::Color::from(c)
}

/// Free space renders as a faint fill so allocated space carries the
/// visual weight.
fn role_alpha(role: SegRole) -> f32 {
    match role {
        SegRole::Free => 0.35,
        SegRole::Existing => 0.8,
        _ => 1.0,
    }
}

fn swatch<'a>(role: SegRole, width: Length, height: f32, radius: f32) -> Element<'a, Message> {
    container(text::body(""))
        .width(width)
        .height(Length::Fixed(height))
        .class(theme::Container::custom(move |t| {
            let mut color = role_color(role, t);
            color.a = role_alpha(role);
            container::Style {
                background: Some(color.into()),
                border: Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

/// Weight per mille of the device, floored so slivers stay visible.
fn weight(size: u64, total: u64) -> u16 {
    (((size as f64 / total as f64) * 1000.0) as u16).max(18)
}

/// The disk map strip. `total_bytes` is the device size — segments are
/// scaled against it, and uncovered space becomes an "unallocated"
/// filler segment so proportions are honest even without gap data.
pub fn bar<'a>(segments: &[BarSegment], total_bytes: u64) -> Element<'a, Message> {
    let covered: u64 = segments.iter().map(|s| s.size_bytes).sum();
    let total = total_bytes.max(covered).max(1);
    // Ignore alignment slack; only show a filler users could act on.
    let remainder_floor = (total / 100).max(64 * 1024 * 1024);
    let remainder = total.saturating_sub(covered);

    let mut r = row::with_capacity(segments.len() + 1).spacing(3);
    for seg in segments {
        r = r.push(swatch(
            seg.role,
            Length::FillPortion(weight(seg.size_bytes, total)),
            26.0,
            6.0,
        ));
    }
    if remainder >= remainder_floor {
        r = r.push(swatch(
            SegRole::Free,
            Length::FillPortion(weight(remainder, total)),
            26.0,
            6.0,
        ));
    }
    container(r).width(Length::Fill).into()
}

/// Legend row: one round color chip + "label · size" per segment, plus
/// the unallocated filler when it's drawn in the bar.
pub fn legend<'a>(segments: &[BarSegment], total_bytes: u64) -> Element<'a, Message> {
    let covered: u64 = segments.iter().map(|s| s.size_bytes).sum();
    let total = total_bytes.max(covered).max(1);
    let remainder_floor = (total / 100).max(64 * 1024 * 1024);
    let remainder = total.saturating_sub(covered);

    let chip = |role: SegRole, label: String, size: u64| -> Element<'a, Message> {
        row::with_capacity(2)
            .spacing(6)
            .align_y(Alignment::Center)
            .push(swatch(role, Length::Fixed(10.0), 10.0, 5.0))
            .push(text::caption(format!("{label} · {}", human_size(size))))
            .into()
    };

    let mut r = row::with_capacity(segments.len() + 1).spacing(16);
    for seg in segments {
        r = r.push(chip(seg.role, seg.label.clone(), seg.size_bytes));
    }
    if remainder >= remainder_floor {
        r = r.push(chip(SegRole::Free, "unallocated".into(), remainder));
    }
    container(r.wrap()).width(Length::Fill).into()
}

/// Caption + bar + legend, the standard stack.
pub fn view<'a>(
    caption: &'a str,
    segments: &[BarSegment],
    total_bytes: u64,
) -> Element<'a, Message> {
    column::with_capacity(3)
        .spacing(8)
        .push(text::caption_heading(caption))
        .push(bar(segments, total_bytes))
        .push(legend(segments, total_bytes))
        .into()
}

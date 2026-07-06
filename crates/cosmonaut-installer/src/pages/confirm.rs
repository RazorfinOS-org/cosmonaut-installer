use cosmic::iced::Length;
use cosmic::widget::{button, column, row, scrollable, settings, text};
use cosmic::Element;

use crate::app::Message;
use crate::disks::{Disk, DiskExt};
use crate::images_json::ImageOption;
use crate::pages::layout::LayoutUiState;
use crate::pages::{wizard_frame, Page};
use crate::spec::{EncryptionChoice, PartitionModeChoice};
use crate::widgets::disk_bar::{self, BarSegment, SegRole};

pub fn view<'a>(
    image: Option<&'a ImageOption>,
    disk: Option<&'a Disk>,
    mode: PartitionModeChoice,
    layout: &'a LayoutUiState,
    encryption: &EncryptionChoice,
    hostname: &'a str,
) -> Element<'a, Message> {
    let disk_label = disk.map(DiskExt::label).unwrap_or_else(|| "?".into());
    let image_label = image.map(|i| i.name.as_str()).unwrap_or("?").to_owned();

    let summary = settings::section()
        .title("Summary")
        .add(settings::item::builder("Image").control(text::body(image_label)))
        .add(settings::item::builder("Disk").control(text::body(disk_label)))
        .add(settings::item::builder("Mode").control(text::body(mode.label().to_owned())))
        .add(
            settings::item::builder("Encryption")
                .control(text::body(encryption.label().to_owned())),
        )
        .add(settings::item::builder("Hostname").control(text::body(hostname.to_owned())));

    let mut body = column::with_capacity(4).spacing(24).push(summary);

    // Before/after layout preview derived from the actual plan.
    if let Some(d) = disk {
        let current = disk_bar::segments_current(d);
        if !current.is_empty() {
            body = body.push(disk_bar::view("Current contents", &current, d.size_bytes));
        }
        let planned = match mode {
            PartitionModeChoice::EraseDisk => disk_bar::segments_erase(d),
            PartitionModeChoice::FreeSpace => segments_free_space(d),
            PartitionModeChoice::Custom => layout.planned_segments(),
        };
        if !planned.is_empty() {
            body = body.push(disk_bar::view("After install", &planned, d.size_bytes));
        }
    }

    let description = match mode {
        PartitionModeChoice::EraseDisk => {
            // Name what's being destroyed when we know it.
            let destroyed: Vec<String> = disk
                .map(|d| {
                    d.partitions
                        .iter()
                        .filter_map(|p| p.detected_os.as_ref())
                        .map(|os| os.pretty_name.clone())
                        .collect()
                })
                .unwrap_or_default();
            if destroyed.is_empty() {
                "Clicking Install will erase the chosen disk and write the chosen image. \
                 This cannot be undone."
                    .to_owned()
            } else {
                format!(
                    "Clicking Install will erase the chosen disk — including {} — and \
                     write the chosen image. This cannot be undone.",
                    destroyed.join(", ")
                )
            }
        }
        PartitionModeChoice::FreeSpace => {
            "Only the shown free space will be used; existing partitions are not modified."
                .to_owned()
        }
        PartitionModeChoice::Custom => {
            "Partitions will be modified as shown. Deleted and formatted partitions \
             cannot be recovered."
                .to_owned()
        }
    };

    // The warning line is owned (it may name detected OSes), so it goes
    // into the body rather than wizard_frame's borrowed description.
    body = body.push(text::body(description));

    let nav = row::with_capacity(2)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(button::destructive("Install").on_press(Message::StartInstall));

    wizard_frame(
        "Confirm",
        None,
        scrollable(body)
            .height(Length::Fill)
            .width(Length::Fill)
            .into(),
        nav.into(),
        Page::Confirm,
    )
}

/// Planned segments for free-space mode: existing partitions kept,
/// largest gap becomes (ESP-if-needed +) root.
fn segments_free_space(disk: &Disk) -> Vec<BarSegment> {
    use cosmonaut_engine::{ESP_BYTES, ROOT_MIN_BYTES};

    let Some(gap) = disk
        .largest_gap()
        .filter(|g| g.size_bytes >= ROOT_MIN_BYTES)
    else {
        return Vec::new();
    };
    const GPT_ESP: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
    let has_esp = disk
        .partitions
        .iter()
        .any(|p| p.part_type.as_deref() == Some(GPT_ESP));

    let mut segments = Vec::new();
    for p in &disk.partitions {
        let name = p
            .label
            .clone()
            .or_else(|| p.fstype.clone())
            .unwrap_or_else(|| format!("partition {}", p.number));
        let role = if p.part_type.as_deref() == Some(GPT_ESP) {
            SegRole::Esp
        } else {
            SegRole::Existing
        };
        segments.push((
            p.start_bytes,
            BarSegment {
                label: name,
                size_bytes: p.size_bytes,
                role,
            },
        ));
    }
    let mut start = gap.start_bytes;
    let mut remaining = gap.size_bytes;
    if !has_esp {
        segments.push((
            start,
            BarSegment {
                label: "EFI".into(),
                size_bytes: ESP_BYTES,
                role: SegRole::Esp,
            },
        ));
        start += ESP_BYTES;
        remaining -= ESP_BYTES;
    }
    segments.push((
        start,
        BarSegment {
            label: "root".into(),
            size_bytes: remaining,
            role: SegRole::Root,
        },
    ));
    // Other gaps stay free.
    for g in &disk.gaps {
        if g.start_bytes != gap.start_bytes {
            segments.push((
                g.start_bytes,
                BarSegment {
                    label: "free space".into(),
                    size_bytes: g.size_bytes,
                    role: SegRole::Free,
                },
            ));
        }
    }
    segments.sort_by_key(|(s, _)| *s);
    segments.into_iter().map(|(_, s)| s).collect()
}

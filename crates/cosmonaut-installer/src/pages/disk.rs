use cosmic::iced::Length;
use cosmic::widget::{self, button, radio, row, scrollable, settings, text};
use cosmic::Element;

use cosmonaut_engine::probe::human_size;
use cosmonaut_engine::ROOT_MIN_BYTES;

use crate::app::Message;
use crate::disks::{Disk, DiskExt};
use crate::pages::{wizard_frame, Page};
use crate::spec::PartitionModeChoice;
use crate::widgets::disk_bar;

pub fn view<'a>(
    disks: &'a [Disk],
    selected: Option<usize>,
    mode: PartitionModeChoice,
    experimental_layout: bool,
) -> Element<'a, Message> {
    if disks.is_empty() {
        let nav = row::with_capacity(2)
            .spacing(12)
            .push(button::standard("Back").on_press(Message::Back))
            .push(button::suggested("Refresh").on_press(Message::RefreshDisks));
        return wizard_frame(
            "Choose a disk",
            Some(
                "No whole-disk block devices were found. Connect a target \
                 and click Refresh.",
            ),
            widget::column::with_capacity(0).into(),
            nav.into(),
            Page::Disk,
        );
    }

    let mut body = widget::column::with_capacity(4).spacing(20);

    let mut section = settings::section().title("Detected disks");
    for (idx, d) in disks.iter().enumerate() {
        let detail = if d.partitions.is_empty() {
            "blank disk".to_owned()
        } else {
            // Detected OSes are the most useful identifiers; fall back
            // to labels/filesystems.
            let named: Vec<String> = d
                .partitions
                .iter()
                .filter_map(|p| {
                    p.detected_os
                        .as_ref()
                        .map(|os| os.pretty_name.clone())
                        .or_else(|| p.label.clone())
                        .or_else(|| p.fstype.clone())
                })
                .take(4)
                .collect();
            let mut s = format!("{} partition(s)", d.partitions.len());
            if !named.is_empty() {
                s.push_str(&format!(": {}", named.join(", ")));
            }
            if let Some(g) = d.largest_gap() {
                s.push_str(&format!(" — {} free", human_size(g.size_bytes)));
            }
            s
        };
        let icon_name = if d.removable {
            "drive-removable-media-symbolic"
        } else {
            "drive-harddisk-system-symbolic"
        };
        section = section.add(
            widget::column::with_capacity(2)
                .spacing(2)
                .push(radio(
                    row::with_capacity(2)
                        .spacing(10)
                        .align_y(cosmic::iced::Alignment::Center)
                        .push(widget::icon::from_name(icon_name).size(20))
                        .push(text::body(d.label())),
                    idx,
                    selected,
                    Message::DiskSelected,
                ))
                .push(text::caption(detail)),
        );
    }
    body = body.push(section);

    // Current contents of the selected disk.
    let selected_disk = selected.and_then(|i| disks.get(i));
    if let Some(d) = selected_disk {
        let segments = disk_bar::segments_current(d);
        if !segments.is_empty() {
            body = body.push(disk_bar::view("Current contents", &segments, d.size_bytes));
        }
    }

    // Partitioning-mode selection (experimental modes env-gated).
    if experimental_layout {
        let mut mode_section = settings::section().title("Installation mode");
        mode_section = mode_section.add(radio(
            text::body(PartitionModeChoice::EraseDisk.label()),
            PartitionModeChoice::EraseDisk,
            Some(mode),
            Message::PartitionModeSelected,
        ));
        let free_ok = selected_disk
            .and_then(|d| d.largest_gap())
            .is_some_and(|g| g.size_bytes >= ROOT_MIN_BYTES);
        if free_ok {
            mode_section = mode_section.add(radio(
                text::body(PartitionModeChoice::FreeSpace.label()),
                PartitionModeChoice::FreeSpace,
                Some(mode),
                Message::PartitionModeSelected,
            ));
        } else {
            mode_section = mode_section.add(text::caption(format!(
                "Install into free space: unavailable (needs a {}+ gap)",
                human_size(ROOT_MIN_BYTES)
            )));
        }
        mode_section = mode_section.add(radio(
            text::body(PartitionModeChoice::Custom.label()),
            PartitionModeChoice::Custom,
            Some(mode),
            Message::PartitionModeSelected,
        ));
        body = body.push(mode_section);
    }

    let description = match mode {
        PartitionModeChoice::EraseDisk => "All data on the chosen disk will be erased.",
        PartitionModeChoice::FreeSpace => {
            "The system installs into unallocated space; existing partitions are kept."
        }
        PartitionModeChoice::Custom => "Choose per-partition roles on the next page.",
    };

    let nav = row::with_capacity(3)
        .spacing(12)
        .push(button::standard("Back").on_press(Message::Back))
        .push(button::standard("Refresh").on_press(Message::RefreshDisks))
        .push(button::suggested("Continue").on_press_maybe(selected.map(|_| Message::Next)));

    wizard_frame(
        "Choose a disk",
        Some(description),
        scrollable(body)
            .height(Length::Fill)
            .width(Length::Fill)
            .into(),
        nav.into(),
        Page::Disk,
    )
}

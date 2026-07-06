//! Wifi support via `iwctl` (iwd's CLI). The daemon mediates because
//! iwd needs root for credential writes; the GUI is unprivileged.
//!
//! We shell out to iwctl rather than talk to iwd's DBus directly: iwd's
//! Connect flow uses an Agent callback for the passphrase, which adds
//! noticeable boilerplate for what's a one-shot wizard interaction.
//! Parsing iwctl's tabular output is the cost; mitigated by being
//! tolerant (best-effort token split, skip malformed lines).

use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WifiNetwork {
    pub ssid: String,
    /// "open", "psk", "8021x", "wep" — whatever iwctl prints.
    pub security: String,
    /// Signal strength, 1..=4 (count of `*`s in iwctl output).
    pub signal: u8,
    /// True if iwd's "currently connected" marker (`>`) was set on the row.
    pub connected: bool,
}

/// Detect a TPM2 device by walking /sys/class/tpm/. Doesn't actually
/// communicate with it (no `tpm2_pcrread` etc.) — just confirms the
/// kernel sees something. Used by the GUI to gray out TPM2-LUKS radio
/// options on hosts without a TPM.
pub fn is_tpm2_available() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/tpm") else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(s) = name.to_str() else { continue };
        if !s.starts_with("tpm") {
            continue;
        }
        // "tpm_version_major" reads "2" for TPM2 devices.
        let path = entry.path().join("tpm_version_major");
        if let Ok(v) = std::fs::read_to_string(&path) {
            if v.trim() == "2" {
                return true;
            }
        }
    }
    false
}

/// Detect any default IPv4 route. Used by the GUI to auto-skip the wifi
/// page when the live env is already online (typical for the wired QEMU
/// path).
pub async fn is_online() -> Result<bool> {
    let out = Command::new("ip")
        .args(["-4", "route", "show", "default"])
        .output()
        .await
        .context("ip route show default")?;
    Ok(!out.stdout.is_empty())
}

/// Return the first wireless device name iwd knows about (e.g. "wlan0").
/// `Ok(None)` means iwd is up but no wireless devices are present
/// (laptop with wifi disabled, or VM with no wireless NIC).
pub async fn first_wireless_device() -> Result<Option<String>> {
    let out = run_iwctl(&["device", "list"]).await?;
    for line in out.lines() {
        // iwctl device list rows look like:
        //   wlan0    phy0    station    on
        let trimmed = line.trim_start_matches([' ', '>']);
        let toks: Vec<&str> = trimmed.split_whitespace().collect();
        if toks.len() >= 4 && toks[3] == "on" && toks[0].starts_with("wl") {
            return Ok(Some(toks[0].to_owned()));
        }
    }
    Ok(None)
}

/// Trigger a scan on the given wireless device, then enumerate visible
/// networks. Caller is expected to have already determined the device.
pub async fn scan(device: &str) -> Result<Vec<WifiNetwork>> {
    // Best-effort scan trigger; iwctl returns 0 even if a scan is
    // already in progress, so we don't error on non-zero here.
    let _ = run_iwctl(&["station", device, "scan"]).await;
    // Brief settle so the scan has a chance to populate the bss list.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let out = run_iwctl(&["station", device, "get-networks"]).await?;
    Ok(parse_networks(&out))
}

/// Connect to `ssid` with `passphrase`. iwctl handles the iwd Agent
/// dance internally; we just drive it with `--passphrase`. Blocks
/// until iwctl returns (success or failure).
pub async fn connect(device: &str, ssid: &str, passphrase: &str) -> Result<()> {
    let out = Command::new("iwctl")
        .args([
            "--dont-ask",
            &format!("--passphrase={passphrase}"),
            "station",
            device,
            "connect",
            ssid,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning iwctl connect")?;
    if !out.status.success() {
        bail!(
            "iwctl connect failed: {}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

async fn run_iwctl(args: &[&str]) -> Result<String> {
    let out = Command::new("iwctl")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("spawning iwctl {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "iwctl {} exited with {}; stderr: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `iwctl station <iface> get-networks` output. The format is:
///
/// ```text
///                               Available networks
/// --------------------------------------------------------------------------------
///   Network name                          Security              Signal
/// --------------------------------------------------------------------------------
/// > homewifi                              psk                   ****
///   homewifi-5g                           psk                   ***
///   guestwifi                             open                  **
/// ```
///
/// Strategy: skip until we've seen the second `---` separator (the
/// underlines bracketing the column header), then for each subsequent
/// non-empty line treat the last two whitespace-delimited tokens as
/// `<security> <signal>` and everything else as the SSID (which may
/// itself contain spaces).
fn parse_networks(stdout: &str) -> Vec<WifiNetwork> {
    let mut out = Vec::new();
    let mut sep_count = 0;
    for line in stdout.lines() {
        let trimmed_for_sep = line.trim();
        if trimmed_for_sep.starts_with("---") {
            sep_count += 1;
            continue;
        }
        if sep_count < 2 {
            continue;
        }
        let connected = line.trim_start().starts_with('>');
        let body = line
            .trim_start_matches([' ', '>'])
            .trim();
        if body.is_empty() {
            continue;
        }
        let toks: Vec<&str> = body.split_whitespace().collect();
        if toks.len() < 3 {
            continue;
        }
        let signal_str = toks[toks.len() - 1];
        let security = toks[toks.len() - 2].to_owned();
        let ssid = toks[..toks.len() - 2].join(" ");
        // Signal column is `*`-padded; count them, cap at 4.
        let signal = signal_str.chars().filter(|c| *c == '*').count().min(4) as u8;
        if signal == 0 || ssid.is_empty() {
            continue;
        }
        out.push(WifiNetwork {
            ssid,
            security,
            signal,
            connected,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_get_networks_output() {
        let sample = "\
                              Available networks\n\
--------------------------------------------------------------------------------\n  \
  Network name                          Security              Signal\n\
--------------------------------------------------------------------------------\n\
> homewifi                              psk                   ****\n  \
  homewifi 5g                           psk                   ***\n  \
  guestwifi                             open                  **\n";
        let nets = parse_networks(sample);
        assert_eq!(nets.len(), 3);
        assert_eq!(nets[0].ssid, "homewifi");
        assert_eq!(nets[0].security, "psk");
        assert_eq!(nets[0].signal, 4);
        assert!(nets[0].connected);
        assert_eq!(nets[1].ssid, "homewifi 5g"); // SSID with space
        assert_eq!(nets[2].security, "open");
    }

    #[test]
    fn parses_no_networks() {
        let sample = "                              Available networks\n----\n  Network name  Security  Signal\n----\n";
        assert!(parse_networks(sample).is_empty());
    }
}

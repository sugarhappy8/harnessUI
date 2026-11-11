//! The graphics cards the operating system reports, independent of any llama.cpp backend.
//!
//! The backend probes (`llama-server --list-devices`) say which devices a backend can
//! use. When a probe lists nothing, that alone does not say why, and it used to hide the
//! card from the rest of the app as well: the ROCm and CUDA downloads were refused on a PC
//! whose Vulkan driver did not answer. Windows knows which cards are installed whatever
//! their drivers do, so the app asks it too.

use serde::Serialize;

/// A display adapter as Windows lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    pub name: String,
    pub driver_version: Option<String>,
}

/// Adapters that are not graphics cards: remote sessions, virtual machines, the fallback
/// driver Windows uses when a card has no driver, and virtual monitors.
fn is_virtual(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "microsoft basic display",
        "microsoft basic render",
        "microsoft remote display",
        "microsoft hyper-v",
        "hyper-v video",
        "vmware",
        "virtualbox",
        "parallels",
        "qxl",
        "virtio",
        "citrix",
        "parsec",
        "virtual display",
        "virtual monitor",
        "indirect display",
        "displaylink",
        "spacedesk",
        "meta virtual",
        "oculus virtual",
        "teamviewer",
        "anydesk",
        "splashtop",
    ]
    .iter()
    .any(|needle| n.contains(needle))
}

/// The names of the real graphics cards among `adapters`, without repeats.
pub fn card_names(adapters: &[Adapter]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in adapters {
        let name = a.name.trim();
        if !name.is_empty() && !is_virtual(name) && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// An AMD Radeon RX 9000-series card (the only ones the pinned ROCm build supports).
pub fn is_rx_9000(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("rx 90") || n.contains("gfx1200") || n.contains("gfx1201")
}

/// An NVIDIA card.
pub fn is_nvidia(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("nvidia") || n.contains("geforce") || n.contains("rtx") || n.contains("quadro")
}

/// Whether this computer can use a downloadable backend, with the reason when it cannot
/// (or a caution when it is offered without knowing). `cards` are the graphics cards known
/// from Windows and from the backends that answered; the CPU is not one of them.
pub fn pack_availability(
    backend: runtime::BackendKind,
    cards: &[String],
) -> (bool, Option<String>) {
    match backend {
        runtime::BackendKind::Rocm => {
            if cards.is_empty() {
                (
                    true,
                    Some("The graphics card could not be identified; this build supports only AMD Radeon RX 9000-series cards.".into()),
                )
            } else if cards.iter().any(|c| is_rx_9000(c)) {
                (true, None)
            } else {
                (
                    false,
                    Some("This ROCm build supports only AMD Radeon RX 9000-series cards.".into()),
                )
            }
        }
        runtime::BackendKind::Cuda => {
            if cards.is_empty() {
                (
                    true,
                    Some(
                        "The graphics card could not be identified; CUDA needs an NVIDIA card."
                            .into(),
                    ),
                )
            } else if cards.iter().any(|c| is_nvidia(c)) {
                (true, None)
            } else {
                (false, Some("CUDA needs an NVIDIA graphics card.".into()))
            }
        }
        _ => (true, None),
    }
}

/// What to tell the user when the Vulkan backend listed no device.
///
/// `loader_present`: the Vulkan loader library is installed. `adapters`: what the
/// operating system reports (empty where the app cannot ask).
pub fn vulkan_no_device_message(loader_present: bool, adapters: &[Adapter]) -> String {
    let cards = card_names(adapters);
    if !cards.is_empty() {
        let seen = cards.join(", ");
        let driver = if loader_present {
            "its Vulkan driver listed no device"
        } else {
            "no Vulkan driver is installed"
        };
        return format!(
            "Windows sees {seen}, but {driver}, so models run on the CPU for now. Install the latest graphics driver from the card's maker (AMD Adrenalin, NVIDIA or Intel), restart the PC, then choose Check again. If that does not help, run the backend check in Settings → Diagnostics and send the report."
        );
    }
    if let Some(only) = adapters.first() {
        return format!(
            "No graphics card that can run models was found: Windows reports only \"{}\". That is usual in a virtual machine, in some remote desktop sessions, or when the card's driver is not installed. Models run on the CPU.",
            only.name.trim()
        );
    }
    if !loader_present {
        return "No Vulkan driver was found. Install or update your graphics driver (AMD Adrenalin, NVIDIA or Intel), then choose Check again. Models run on the CPU until then.".into();
    }
    "No graphics card that Vulkan can use was found. Install or update your graphics driver (AMD Adrenalin, NVIDIA or Intel), then choose Check again. Models run on the CPU until then.".into()
}

/// The display adapters Windows lists (empty on other systems, where the backends'
/// own lists are enough).
pub fn adapters() -> Vec<Adapter> {
    #[cfg(windows)]
    {
        windows_adapters()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// From the display adapter class in the registry: one numbered key per adapter, with
/// `DriverDesc` (the name Device Manager shows) and `DriverVersion`.
#[cfg(windows)]
fn windows_adapters() -> Vec<Adapter> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::PCWSTR;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read(subkey: &str, value: &str) -> Option<String> {
        let key = wide(subkey);
        let name = wide(value);
        let mut buf = vec![0u16; 512];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `key` and `name` are NUL-terminated and outlive the call; `buf` has
        // `size` bytes of space and RegGetValueW writes at most that many, NUL-terminated
        // (RRF_RT_REG_SZ).
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status.is_err() {
            return None;
        }
        let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..chars]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    let class = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let mut out = Vec::new();
    // Keys are numbered 0000, 0001, ...; removed adapters can leave gaps.
    for index in 0..32 {
        let key = format!(r"{class}\{index:04}");
        if let Some(name) = read(&key, "DriverDesc") {
            out.push(Adapter {
                name,
                driver_version: read(&key, "DriverVersion"),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::BackendKind;

    fn adapter(name: &str) -> Adapter {
        Adapter {
            name: name.into(),
            driver_version: Some("32.0.1".into()),
        }
    }

    #[test]
    fn real_cards_are_told_from_virtual_adapters() {
        let all = [
            adapter("AMD Radeon RX 9070 XT"),
            adapter("AMD Radeon(TM) Graphics"),
            adapter("Microsoft Basic Display Adapter"),
            adapter("Microsoft Remote Display Adapter"),
            adapter("Parsec Virtual Display Adapter"),
            adapter("AMD Radeon RX 9070 XT"),
            adapter("  "),
        ];
        assert_eq!(
            card_names(&all),
            vec!["AMD Radeon RX 9070 XT", "AMD Radeon(TM) Graphics"]
        );
        assert!(card_names(&[adapter("Microsoft Hyper-V Video")]).is_empty());
        assert_eq!(
            card_names(&[
                adapter("NVIDIA GeForce RTX 4070"),
                adapter("Intel(R) UHD Graphics 770")
            ])
            .len(),
            2
        );
    }

    /// The bug behind "This ROCm build supports only RX 9000-series cards" on a PC with
    /// an RX 9070 XT: the card was known only through Vulkan, and the CPU counted as a
    /// card, so a silent Vulkan driver made the app conclude the card was something else.
    #[test]
    fn a_download_is_offered_from_what_windows_reports() {
        let amd = vec!["AMD Radeon RX 9070 XT".to_string()];
        assert_eq!(pack_availability(BackendKind::Rocm, &amd), (true, None));
        let (ok, why) = pack_availability(BackendKind::Cuda, &amd);
        assert!(!ok && why.unwrap().contains("NVIDIA"));

        let nvidia = vec!["NVIDIA GeForce RTX 4070".to_string()];
        assert_eq!(pack_availability(BackendKind::Cuda, &nvidia), (true, None));
        let (ok, why) = pack_availability(BackendKind::Rocm, &nvidia);
        assert!(!ok && why.unwrap().contains("RX 9000"));

        // An older Radeon is not offered ROCm.
        let old = vec!["AMD Radeon RX 7900 XTX".to_string()];
        assert!(!pack_availability(BackendKind::Rocm, &old).0);

        // Nothing known: offered with a caution, never refused for a card nobody saw.
        for kind in [BackendKind::Rocm, BackendKind::Cuda] {
            let (ok, why) = pack_availability(kind, &[]);
            assert!(ok, "{kind:?}");
            assert!(why.unwrap().contains("could not be identified"));
        }
    }

    #[test]
    fn the_vulkan_message_says_what_windows_sees_and_what_to_do() {
        // The card is there but its driver did not answer.
        let m = vulkan_no_device_message(true, &[adapter("AMD Radeon RX 9070 XT")]);
        assert!(m.contains("Windows sees AMD Radeon RX 9070 XT"), "{m}");
        assert!(
            m.contains("Vulkan driver listed no device") && m.contains("Check again"),
            "{m}"
        );
        assert!(m.contains("CPU"), "{m}");
        // No Vulkan driver at all.
        let m = vulkan_no_device_message(false, &[adapter("Intel(R) HD Graphics 4000")]);
        assert!(m.contains("no Vulkan driver is installed"), "{m}");
        // Only a virtual adapter: say so instead of blaming the driver.
        let m = vulkan_no_device_message(true, &[adapter("Microsoft Basic Display Adapter")]);
        assert!(
            m.contains("Windows reports only \"Microsoft Basic Display Adapter\""),
            "{m}"
        );
        assert!(m.contains("virtual machine"), "{m}");
        // Nothing known (not Windows).
        assert!(vulkan_no_device_message(false, &[]).starts_with("No Vulkan driver was found"));
        assert!(
            vulkan_no_device_message(true, &[]).contains("Install or update your graphics driver")
        );
    }
}

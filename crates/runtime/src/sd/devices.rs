//! The Studio engine's devices (`sd-cli --list-devices`), matched to llama.cpp's
//! (docs/dev/0.1.0/studio-local-design.md, L2).
//!
//! The pinned CLI prints one `name<TAB>description` a line on stdout, every ggml device
//! including the processor (`CPU`), and exits 0: `Vulkan0<TAB>AMD Radeon RX 7600 (RADV
//! NAVI33)`, then `CPU<TAB>AMD Ryzen 7 7700X 8-Core Processor`.
//!
//! Studio knows the graphics card from llama.cpp's probe, so the engine's device is found by
//! the card's description: index, order and the name's case can differ between the two.

/// One device row of `sd-cli --list-devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdDevice {
    /// The ggml device name (`Vulkan0`, `MTL0`, `CPU`), which `--backend` matches without
    /// regard to case.
    pub name: String,
    /// The device's description (`AMD Radeon RX 7600 (RADV NAVI33)`).
    pub description: String,
}

/// Parses `sd-cli --list-devices` output. A line without a tab, with an empty name or a name
/// holding white space is skipped; CRLF endings are read too.
pub fn parse_list_devices(stdout: &str) -> Vec<SdDevice> {
    stdout
        .lines()
        .filter_map(|line| {
            let (name, description) = line.split_once('\t')?;
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            Some(SdDevice {
                name: name.to_owned(),
                description: description.trim().to_owned(),
            })
        })
        .collect()
}

/// The `--backend` word for llama.cpp's device described as `llama_description`: the first
/// of `devices` with that description, compared without regard to case or surrounding white
/// space, its name lower-cased (`vulkan0`, the form the pinned CLI's help uses). The
/// processor is never matched: on a Mac it shares the graphics part's description. `None`
/// when no device matches, or the description is blank.
pub fn match_llama_device(devices: &[SdDevice], llama_description: &str) -> Option<String> {
    let wanted = llama_description.trim().to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    devices
        .iter()
        .filter(|device| !device.name.eq_ignore_ascii_case("cpu"))
        .find(|device| device.description.trim().to_lowercase() == wanted)
        .map(|device| device.name.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VULKAN: &str = include_str!("../../tests/fixtures/sd_list_devices_vulkan.txt");

    #[test]
    fn devices_match_llamas_by_description() {
        let d = parse_list_devices(VULKAN);
        assert_eq!(
            match_llama_device(&d, "AMD Radeon RX 7600 (RADV NAVI33)").as_deref(),
            Some("vulkan0")
        );
        assert_eq!(match_llama_device(&d, "NVIDIA GeForce RTX 4070"), None);
        // Case and surrounding spaces do not matter; the integrated card is the second.
        assert_eq!(
            match_llama_device(&d, " amd radeon graphics (radv raphael_mendocino) ").as_deref(),
            Some("vulkan1")
        );
    }

    #[test]
    fn each_line_is_a_name_a_tab_and_a_description() {
        assert_eq!(
            parse_list_devices(VULKAN),
            [
                ("Vulkan0", "AMD Radeon RX 7600 (RADV NAVI33)"),
                ("Vulkan1", "AMD Radeon Graphics (RADV RAPHAEL_MENDOCINO)"),
                ("CPU", "AMD Ryzen 7 7700X 8-Core Processor"),
            ]
            .map(|(name, description)| SdDevice {
                name: name.into(),
                description: description.into(),
            })
        );
        // CRLF endings, a line without a tab, a blank line and a name with spaces are skipped.
        let text = "Vulkan0\tNVIDIA GeForce RTX 4070\r\nggml_vulkan: Found 1 Vulkan devices:\r\n\r\nbad name\tx\r\n\tno name\r\nCPU\t\r\n";
        assert_eq!(
            parse_list_devices(text),
            [("Vulkan0", "NVIDIA GeForce RTX 4070"), ("CPU", "")].map(|(name, description)| {
                SdDevice {
                    name: name.into(),
                    description: description.into(),
                }
            })
        );
        assert_eq!(parse_list_devices(""), []);
    }

    /// On a Mac the processor and the graphics part share a description; the processor is
    /// never llama's device. Nor is a blank description anyone's.
    #[test]
    fn the_processor_and_a_blank_description_are_never_matched() {
        let d = parse_list_devices("MTL0\tApple M2 Pro\nBLAS\tAccelerate\nCPU\tApple M2 Pro\n");
        assert_eq!(
            match_llama_device(&d, "Apple M2 Pro").as_deref(),
            Some("mtl0")
        );
        let cpu_only = parse_list_devices("CPU\tApple M2 Pro\n");
        assert_eq!(match_llama_device(&cpu_only, "Apple M2 Pro"), None);
        let unnamed = parse_list_devices("Vulkan0\t\n");
        assert_eq!(match_llama_device(&unnamed, " "), None);
    }
}

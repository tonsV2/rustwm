//! Which XKB keymap to compile. Each field comes from the config file, then
//! the `XKB_DEFAULT_*` variables, then the system keyboard set with
//! `localectl`, so a machine set up for a Danish keyboard types Danish
//! without any configuration.

use std::{env, fs};

use crate::config::KeyboardConfig;

/// XKB rule names. An empty field lets libxkbcommon fill it in from its
/// `XKB_DEFAULT_*` variable, or its built-in default ("us" for the layout).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Names {
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: String,
}

/// Where localed stores the system keyboard: `vconsole.conf` in newer
/// systemd versions, the Xorg snippet in older ones.
const VCONSOLE: &str = "/etc/vconsole.conf";
const XORG_KEYBOARD: &str = "/etc/X11/xorg.conf.d/00-keyboard.conf";

pub fn resolve(config: &KeyboardConfig) -> Names {
    resolve_with(config, |name| env::var(name).ok(), system_names)
}

fn resolve_with(
    config: &KeyboardConfig,
    var: impl Fn(&str) -> Option<String>,
    system: impl FnOnce() -> Names,
) -> Names {
    let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
    // The system keyboard is a whole: its variant and options belong to its
    // layout, so it only applies when nothing else picks the layout.
    let system = if config.layout.is_none() && !set("XKB_DEFAULT_LAYOUT") {
        system()
    } else {
        Names::default()
    };
    let pick = |configured: &Option<String>, name: &str, system: String| match configured {
        Some(value) => value.clone(),
        None if set(name) => String::new(),
        None => system,
    };
    Names {
        model: pick(&config.model, "XKB_DEFAULT_MODEL", system.model),
        layout: pick(&config.layout, "XKB_DEFAULT_LAYOUT", system.layout),
        variant: pick(&config.variant, "XKB_DEFAULT_VARIANT", system.variant),
        options: pick(&config.options, "XKB_DEFAULT_OPTIONS", system.options),
    }
}

fn system_names() -> Names {
    let vconsole = fs::read_to_string(VCONSOLE)
        .map(|text| parse_vconsole(&text))
        .unwrap_or_default();
    if !vconsole.layout.is_empty() {
        return vconsole;
    }
    fs::read_to_string(XORG_KEYBOARD)
        .map(|text| parse_xorg(&text))
        .unwrap_or_default()
}

/// Reads `XKBLAYOUT="dk"` style lines.
fn parse_vconsole(text: &str) -> Names {
    let mut names = Names::default();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let field = match key {
            "XKBMODEL" => &mut names.model,
            "XKBLAYOUT" => &mut names.layout,
            "XKBVARIANT" => &mut names.variant,
            "XKBOPTIONS" => &mut names.options,
            _ => continue,
        };
        *field = value.trim().trim_matches('"').to_string();
    }
    names
}

/// Reads `Option "XkbLayout" "dk"` style lines.
fn parse_xorg(text: &str) -> Names {
    let mut names = Names::default();
    for line in text.lines() {
        let words: Vec<&str> = line.split('"').collect();
        // `Option "XkbLayout" "dk"` splits into ["Option ", "XkbLayout", " ", "dk", ""].
        let [option, key, _, value, ..] = words[..] else {
            continue;
        };
        if option.trim() != "Option" {
            continue;
        }
        let field = match key {
            "XkbModel" => &mut names.model,
            "XkbLayout" => &mut names.layout,
            "XkbVariant" => &mut names.variant,
            "XkbOptions" => &mut names.options,
            _ => continue,
        };
        *field = value.to_string();
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn danish() -> Names {
        Names {
            model: "pc105".into(),
            layout: "dk".into(),
            variant: String::new(),
            options: "caps:escape".into(),
        }
    }

    #[test]
    fn system_keyboard_applies_without_config_or_variables() {
        let names = resolve_with(&KeyboardConfig::default(), |_| None, danish);
        assert_eq!(names, danish());
    }

    #[test]
    fn config_wins_and_drops_the_system_keyboard() {
        let config = KeyboardConfig {
            layout: Some("us,dk".into()),
            ..Default::default()
        };
        let names = resolve_with(&config, |_| None, || panic!("system read"));
        assert_eq!(names.layout, "us,dk");
        assert_eq!(names.options, "");
    }

    #[test]
    fn variables_are_left_to_libxkbcommon() {
        let var = |name: &str| (name == "XKB_DEFAULT_LAYOUT").then(|| "de".to_string());
        let config = KeyboardConfig {
            options: Some("caps:escape".into()),
            ..Default::default()
        };
        let names = resolve_with(&config, var, || panic!("system read"));
        assert_eq!(names.layout, "");
        assert_eq!(names.options, "caps:escape");
    }

    #[test]
    fn vconsole_conf_parses() {
        let names = parse_vconsole("KEYMAP=\"dk\"\nXKBLAYOUT=\"dk\"\nXKBMODEL=pc105\n");
        assert_eq!(names.layout, "dk");
        assert_eq!(names.model, "pc105");
        assert_eq!(parse_vconsole("KEYMAP=\"dk\"\n").layout, "");
    }

    #[test]
    fn xorg_keyboard_conf_parses() {
        let text = "# Written by systemd-localed(8)\nSection \"InputClass\"\n        Identifier \"system-keyboard\"\n        MatchIsKeyboard \"on\"\n        Option \"XkbLayout\" \"dk\"\n        Option \"XkbModel\" \"pc105\"\nEndSection\n";
        let names = parse_xorg(text);
        assert_eq!(names.layout, "dk");
        assert_eq!(names.model, "pc105");
        assert_eq!(names.variant, "");
    }
}

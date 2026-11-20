//! Module system: hostname and OS description (02-system-modules.md section 4,
//! SYS-020..SYS-023).

use std::time::Duration;

use crate::alerts::Alerts;
use crate::config::Config;
use crate::modules::cpu::cfg_refresh;
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{Platform, SystemInfo, Sys};
use crate::ui::panel::{flags, Align, Panel, Role};

const PLACEHOLDERS: [&str; 5] = ["hostname", "os_name", "os_version", "arch", "linux_distro"];
const DEFAULT_DISTRO: &str = "{linux_distro} {arch} / {os_name} {os_version}";
const DEFAULT_PLAIN: &str = "{os_name} {os_version} {arch}";

pub struct System {
    refresh: Duration,
    /// User template, None = platform default (SYS-022).
    template: Option<String>,
    info: Option<SystemInfo>,
    desc: String,
}

impl System {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = (ui, alerts);
        let template = cfg.str(&["system", "info_format"]).and_then(|t| {
            // SYS-022: unknown (or, off Linux, always absent) placeholders -> default + warning.
            let mut bad = None;
            each_placeholder(t, |name| {
                let ok = PLACEHOLDERS.contains(&name) && (cfg!(target_os = "linux") || name != "linux_distro");
                if !ok && bad.is_none() {
                    bad = Some(name.to_string());
                }
            });
            match bad {
                Some(name) => {
                    eprintln!("ogle: [system] info_format: placeholder {{{name}}} is unknown or absent here; using the default");
                    None
                }
                None => Some(t.to_string()),
            }
        });
        System {
            refresh: cfg_refresh(cfg, "system").unwrap_or(Duration::from_secs(60)),
            template,
            info: None,
            desc: String::new(),
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        Some(self.refresh)
    }

    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        let _ = (ctx, ui, alerts);
        let info = sys.system_info();
        let default = if info.distro.is_some() { DEFAULT_DISTRO } else { DEFAULT_PLAIN };
        let mut tpl = self.template.as_deref().unwrap_or(default);
        if info.distro.is_none() {
            let mut needs = false;
            each_placeholder(tpl, |n| needs |= n == "linux_distro");
            if needs {
                tpl = default;
            }
        }
        self.desc.clear();
        let mut rest = tpl;
        while let Some(i) = rest.find('{') {
            self.desc.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            match after.find('}') {
                Some(j) => {
                    let v = match &after[..j] {
                        "hostname" => info.hostname.as_str(),
                        "os_name" => info.os_name.as_str(),
                        "os_version" => info.os_version.as_str(),
                        "arch" => info.arch.as_str(),
                        "linux_distro" => info.distro.as_deref().unwrap_or(""),
                        _ => "",
                    };
                    self.desc.push_str(v);
                    rest = &after[j + 1..];
                }
                None => {
                    self.desc.push_str(&rest[i..]);
                    rest = "";
                }
            }
        }
        self.desc.push_str(rest);
        self.info = Some(info);
    }
}

fn each_placeholder(t: &str, mut f: impl FnMut(&str)) {
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else { break };
        f(&after[..j]);
        rest = &after[j + 1..];
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let s = &w.system;
    let Some(info) = &s.info else { return Align::Top };
    // SYS-021
    out.add(Role::Title, 0, &info.hostname);
    if !area.narrow && !s.desc.is_empty() {
        out.addf(Role::Default, flags::OPTIONAL, format_args!(" {} ", s.desc));
    }
    Align::Top
}

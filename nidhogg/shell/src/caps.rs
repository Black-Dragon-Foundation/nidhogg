//! The capabilities record: one protocol version per optional feature (0 = absent).
//!
//! Every protocol but five is optional; a missing one makes its feature do nothing, never a separate code path.  The
//! required ones are `compositor` >= 4, `subcompositor`, `seat`, `xdg_wm_base` >= 2 and the shm global, which has no
//! field because opening fails without it.

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NhShellCaps {
    pub compositor: u32,
    pub subcompositor: u32,
    pub seat: u32,
    pub xdg_wm_base: u32,
    pub xdg_decoration: u32,
    pub fractional_scale: u32,
    pub viewporter: u32,
    pub cursor_shape: u32,
    pub text_input_v3: u32,
    pub primary_selection: u32,
    pub data_device: u32,
    pub presentation: u32,
    pub fifo: u32,
    pub commit_timing: u32,
    pub alpha_modifier: u32,
    pub activation: u32,
    pub pointer_warp: u32,
    pub system_bell: u32,
    pub session_management: u32,
    pub background_effect: u32,
    pub pointer_gestures: u32,
    pub idle_inhibit: u32,
    pub single_pixel_buffer: u32,
    pub dmabuf: u32,
    /// Server-side decorations were granted, after negotiation.
    pub ssd: bool,
    /// The host forces client-side decorations (a test switch; the recorder's `--force-csd`).
    pub force_csd: bool,
}

type Field = fn(&mut NhShellCaps) -> &mut u32;

/// Registry global (the protocol's interface name) -> the field that holds its version.
pub const GLOBAL_FIELDS: [(&str, Field); 24] = [
    ("wl_compositor", |c| &mut c.compositor),
    ("wl_subcompositor", |c| &mut c.subcompositor),
    ("wl_seat", |c| &mut c.seat),
    ("xdg_wm_base", |c| &mut c.xdg_wm_base),
    ("zxdg_decoration_manager_v1", |c| &mut c.xdg_decoration),
    ("wp_fractional_scale_manager_v1", |c| {
        &mut c.fractional_scale
    }),
    ("wp_viewporter", |c| &mut c.viewporter),
    ("wp_cursor_shape_manager_v1", |c| &mut c.cursor_shape),
    ("zwp_text_input_manager_v3", |c| &mut c.text_input_v3),
    ("zwp_primary_selection_device_manager_v1", |c| {
        &mut c.primary_selection
    }),
    ("wl_data_device_manager", |c| &mut c.data_device),
    ("wp_presentation", |c| &mut c.presentation),
    ("wp_fifo_manager_v1", |c| &mut c.fifo),
    ("wp_commit_timing_manager_v1", |c| &mut c.commit_timing),
    ("wp_alpha_modifier_v1", |c| &mut c.alpha_modifier),
    ("xdg_activation_v1", |c| &mut c.activation),
    ("wp_pointer_warp_v1", |c| &mut c.pointer_warp),
    ("xdg_system_bell_v1", |c| &mut c.system_bell),
    ("xdg_session_manager_v1", |c| &mut c.session_management),
    ("ext_background_effect_manager_v1", |c| {
        &mut c.background_effect
    }),
    ("zwp_pointer_gestures_v1", |c| &mut c.pointer_gestures),
    ("zwp_idle_inhibit_manager_v1", |c| &mut c.idle_inhibit),
    ("wp_single_pixel_buffer_manager_v1", |c| {
        &mut c.single_pixel_buffer
    }),
    ("zwp_linux_dmabuf_v1", |c| &mut c.dmabuf),
];

/// Globals a profile lists that have no field: `wl_shm` is required (opening fails without it) and `wl_output` is
/// tracked per output (output events), not as a capability.
pub const GLOBALS_WITHOUT_FIELD: [&str; 2] = ["wl_shm", "wl_output"];

impl NhShellCaps {
    /// The field for registry global `name`, or `None` for a global the shell does not use.
    pub fn field_mut(&mut self, name: &str) -> Option<&mut u32> {
        GLOBAL_FIELDS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, f)| f(self))
    }

    /// Record that the compositor offers `name` at `version` (the lower of what it offers and what we support is
    /// the caller's business).  Unused globals are ignored.
    pub fn set_global(&mut self, name: &str, version: u32) {
        if let Some(f) = self.field_mut(name) {
            *f = version;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The keys of the `"globals"` object of a recorder profile (no JSON crate: a scan for `"key":` at depth 1).
    fn profile_globals(json: &str) -> Vec<String> {
        let start = json.find("\"globals\"").expect("no \"globals\" in profile");
        let body = &json[start..];
        let open = body.find('{').expect("globals is not an object");
        let (mut depth, mut keys, mut chars) = (0, Vec::new(), body[open..].char_indices());
        while let Some((i, c)) = chars.next() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                '"' if depth == 1 => {
                    let rest = &body[open + i + 1..];
                    let end = rest.find('"').expect("unterminated string");
                    if rest[end + 1..].trim_start().starts_with(':') {
                        keys.push(rest[..end].to_string());
                    }
                    for _ in 0..=end {
                        chars.next();
                    }
                }
                _ => {}
            }
        }
        keys
    }

    #[test]
    fn table_names_distinct_fields() {
        let mut caps = NhShellCaps::default();
        let names: HashSet<&str> = GLOBAL_FIELDS.iter().map(|(n, _)| *n).collect();
        assert_eq!(names.len(), GLOBAL_FIELDS.len());
        let addrs: HashSet<usize> = GLOBAL_FIELDS
            .iter()
            .map(|(_, f)| f(&mut caps) as *mut u32 as usize)
            .collect();
        assert_eq!(addrs.len(), GLOBAL_FIELDS.len(), "two names share a field");
        // all 24 u32 fields are covered: the record is those plus the two flags
        assert_eq!(
            std::mem::size_of::<NhShellCaps>(),
            4 * GLOBAL_FIELDS.len() + 4
        );
    }

    #[test]
    fn profile_names_map() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test/recorder/profiles/mutter.json"
        );
        let json = std::fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("profiles/mutter.json not found (S0 Task 2)"));
        let keys = profile_globals(&json);
        assert!(!keys.is_empty(), "no globals parsed");
        let mut caps = NhShellCaps::default();
        for k in &keys {
            assert!(
                GLOBALS_WITHOUT_FIELD.contains(&k.as_str()) || caps.field_mut(k).is_some(),
                "profile global {k} maps to no NhShellCaps field"
            );
        }
    }
}

//! Template rendering pipeline for mods — ports the C# merger's
//! `PatchTemplateBehaviour` (MediatR pipeline behavior run before patching).
//!
//! Per mod, in order:
//! 1. `_vars` are rendered (each entry sees the previously rendered vars as
//!    `vars.*`; the C# iterates its dictionary, we sort for determinism).
//! 2. Patch sets whose `name` appears in `enableSteps` are dropped iff the
//!    rendered condition parses as `false` (non-boolean renders keep the set).
//! 3. Each patch renders `value` first, then `template`, then `window.after`,
//!    then `window.before`.
//! 4. `filePatches` targets whose set list becomes empty are dropped;
//!    `assetPatches` targets are kept.
//!
//! Bare identifiers (e.g. `{{ mult }}`) render empty — only `vars.x` and
//! `inputs.x` resolve, and hosted-app builtins like `{{DB_Aircraft.X}}` are
//! not provided locally (Fluid non-strict default).

use std::collections::HashMap;

use crate::manifest::{FilePatch, Patch, WingmanMod};
use crate::templating::{self, Vars};

/// Request-level template inputs. CLI builds carry none (the C# merger only
/// fills these from interactive parameter prompts).
fn request_inputs() -> Vars {
    Vars::new()
}

/// Render the mod's `_vars` map into a lookup table.
pub fn render_mod_variables(modm: &WingmanMod) -> Vars {
    let inputs = request_inputs();
    let mut rendered: Vars = Vars::new();
    let mut keys: Vec<&String> = modm.variables.keys().collect();
    keys.sort();
    for key in keys {
        let raw = &modm.variables[key];
        let value = templating::render(raw, &inputs, &rendered);
        rendered.insert(key.clone(), value);
    }
    rendered
}

/// `enableSteps` gate: keep the set unless the rendered condition is `false`.
fn set_enabled(name: &str, steps: &HashMap<String, String>, inputs: &Vars, vars: &Vars) -> bool {
    let Some(cond) = steps.get(name) else {
        return true;
    };
    let rendered = templating::render(cond, inputs, vars);
    rendered.trim().parse::<bool>().unwrap_or(true)
}

fn render_file_patch(patch: &mut FilePatch, inputs: &Vars, vars: &Vars) {
    if let Some(v) = patch.value.clone() {
        patch.value = Some(templating::render(&v, inputs, vars));
    }
    if let Some(t) = patch.template.clone() {
        patch.template = Some(templating::render(&t, inputs, vars));
    }
    if let Some(w) = &mut patch.window {
        if let Some(a) = w.after.clone() {
            w.after = Some(templating::render(&a, inputs, vars));
        }
        if let Some(b) = w.before.clone() {
            w.before = Some(templating::render(&b, inputs, vars));
        }
    }
}

fn render_asset_patch(patch: &mut Patch, inputs: &Vars, vars: &Vars) {
    patch.value = templating::render(&patch.value.clone(), inputs, vars);
    patch.template = templating::render(&patch.template.clone(), inputs, vars);
}

/// Apply the full template pipeline to a mod in place.
pub fn apply_variables_to_mod(modm: &mut WingmanMod) {
    let inputs = request_inputs();
    let vars = render_mod_variables(modm);
    let steps = modm.sicario.enable_steps.clone();

    for sets in modm.file_patches.values_mut() {
        sets.retain(|set| set_enabled(&set.name, &steps, &inputs, &vars));
        for set in sets.iter_mut() {
            for patch in set.patches.iter_mut() {
                render_file_patch(patch, &inputs, &vars);
            }
        }
    }
    modm.file_patches.retain(|_, sets| !sets.is_empty());

    for sets in modm.asset_patches.values_mut() {
        sets.retain(|set| set_enabled(&set.name, &steps, &inputs, &vars));
        for set in sets.iter_mut() {
            for patch in set.patches.iter_mut() {
                render_asset_patch(patch, &inputs, &vars);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_mod_json;

    #[test]
    fn test_simple_substitution() {
        let json = r#"{
            "_id": "t", "_vars": { "damage_mult": "2.0" },
            "assetPatches": { "x": [{ "name": "s", "patches": [
                { "template": "FloatProperty:{{ vars.damage_mult }}", "value": "FloatProperty:{{ vars.damage_mult }}", "type": "propertyValue" }
            ]}]}
        }"#;
        let mut m = parse_mod_json(json).unwrap();
        apply_variables_to_mod(&mut m);
        let patch = &m.asset_patches["x"][0].patches[0];
        assert_eq!(patch.template, "FloatProperty:2.0");
    }

    #[test]
    fn bare_identifier_renders_empty() {
        // Faithful Fluid behavior: only `vars.x` / `inputs.x` resolve.
        let json = r#"{
            "_id": "t", "_vars": { "mult": "2.0" },
            "assetPatches": { "x": [{ "name": "s", "patches": [
                { "template": "{{ mult }}", "value": "{{ mult }}", "type": "propertyValue" }
            ]}]}
        }"#;
        let mut m = parse_mod_json(json).unwrap();
        apply_variables_to_mod(&mut m);
        assert_eq!(m.asset_patches["x"][0].patches[0].value, "");
    }

    #[test]
    fn builtin_vars_render_empty() {
        // Hosted-app builtins are not provided by the local merger.
        let json = r#"{
            "_id": "t",
            "filePatches": { "f.uexp": [{ "name": "s", "patches": [
                { "template": "ED {{DB_Aircraft.CannonType}}", "value": "EE {{DB_Aircraft.CannonType}}", "type": "inPlace" }
            ]}]}
        }"#;
        let mut m = parse_mod_json(json).unwrap();
        apply_variables_to_mod(&mut m);
        let patch = &m.file_patches["f.uexp"][0].patches[0];
        assert_eq!(patch.template.as_deref(), Some("ED "));
        assert_eq!(patch.value.as_deref(), Some("EE "));
    }

    #[test]
    fn substitution_field_is_ignored() {
        // Doc-style `substitution` does not bind to `value` (C# contract).
        let json = r#"{
            "_id": "t",
            "filePatches": { "f.uexp": [{ "name": "s", "patches": [
                { "template": "00 48 02", "substitution": "01", "type": "inPlace" }
            ]}]}
        }"#;
        let m = parse_mod_json(json).unwrap();
        let patch = &m.file_patches["f.uexp"][0].patches[0];
        assert_eq!(patch.value, None);
    }

    #[test]
    fn steps_skip_false() {
        let json = r#"{
            "_id": "t",
            "_sicario": { "enableSteps": { "ReplaceAny": "false" } },
            "assetPatches": { "x": [{ "name": "ReplaceAny", "patches": [
                { "template": "datatable:[*]", "value": "BoolProperty:true", "type": "propertyValue" }
            ]}]}
        }"#;
        let mut m = parse_mod_json(json).unwrap();
        apply_variables_to_mod(&mut m);
        assert!(m.asset_patches["x"].is_empty());
    }

    #[test]
    fn steps_keep_true_and_non_bool() {
        let json = r#"{
            "_id": "t",
            "_sicario": { "enableSteps": { "A": "true", "B": "not-a-bool" } },
            "assetPatches": { "x": [
                { "name": "A", "patches": [ { "template": "t", "value": "v", "type": "propertyValue" } ] },
                { "name": "B", "patches": [ { "template": "t", "value": "v", "type": "propertyValue" } ] }
            ]}
        }"#;
        let mut m = parse_mod_json(json).unwrap();
        apply_variables_to_mod(&mut m);
        assert_eq!(m.asset_patches["x"].len(), 2);
    }

    #[test]
    fn file_targets_dropped_when_all_sets_skipped() {
        let json = r#"{
            "_id": "t",
            "_sicario": { "enableSteps": { "s": "false" } },
            "filePatches": { "f.uexp": [{ "name": "s", "patches": [
                { "template": "00", "value": "01", "type": "inPlace" }
            ]}]}
        }"#;
        let mut m = parse_mod_json(json).unwrap();
        apply_variables_to_mod(&mut m);
        assert!(m.file_patches.is_empty());
    }

    #[test]
    fn cross_variable_render() {
        let json = r#"{
            "_id": "t",
            "_vars": { "a": "hello", "b": "{{ vars.a }} world" },
            "assetPatches": { "x": [{ "name": "s", "patches": [
                { "template": "{{ vars.b }}", "value": "{{ vars.b }}", "type": "propertyValue" }
            ]}]}
        }"#;
        let mut m = parse_mod_json(json).unwrap();
        apply_variables_to_mod(&mut m);
        // Sorted order renders `a` before `b`.
        assert_eq!(m.asset_patches["x"][0].patches[0].template, "hello world");
    }
}

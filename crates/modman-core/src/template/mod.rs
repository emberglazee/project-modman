//! Template substitution engine for patch values.
//!
//! Supports `{{ variable }}` syntax in patch values and template strings,
//! with variables coming from `_vars` and `_inputs` in the WingmanMod JSON.

use std::collections::HashMap;

/// Substitute template variables in a string
///
/// Replaces `{{ varname }}` with the corresponding value from `vars`.
/// Unknown variables are left as-is (with a warning).
pub fn substitute(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '{' && chars[i + 1] == '{' {
            i += 2; // skip {{
                    // Skip whitespace
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            // Read variable name
            let start = i;
            while i < chars.len() && chars[i] != '}' {
                i += 1;
            }
            let raw: String = chars[start..i].iter().collect();
            let var_name = raw.trim().to_string();
            // Skip }}
            if i + 1 < chars.len() && chars[i] == '}' && chars[i + 1] == '}' {
                i += 2;
            }

            // Liquid-style namespaced names: `vars.x` reads the variables map;
            // `inputs.x` is resolved by the enableSteps logic (left untouched
            // here, matching the C# templating behavior).
            let lookup = var_name.strip_prefix("vars.").unwrap_or(&var_name);
            match vars.get(lookup) {
                Some(val) => result.push_str(val),
                None => {
                    // Leave as-is
                    result.push_str(&format!("{{{{ {} }}}}", var_name));
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Apply template variables to all parts of a WingmanMod
pub fn apply_variables_to_mod(modm: &mut crate::manifest::WingmanMod) {
    let vars = &modm.variables;

    // Substitute in all patch values and templates
    for patches in modm.asset_patches.values_mut() {
        for set in patches.iter_mut() {
            for patch in set.patches.iter_mut() {
                patch.template = substitute(&patch.template, vars);
                patch.value = substitute(&patch.value, vars);
            }
        }
    }

    for patches in modm.file_patches.values_mut() {
        for set in patches.iter_mut() {
            for patch in set.patches.iter_mut() {
                patch.template = substitute(&patch.template, vars);
                patch.substitution = substitute(&patch.substitution, vars);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_substitution() {
        let vars = [("damage_mult".into(), "2.0".into())].into();
        let result = substitute("FloatProperty:{{ damage_mult }}", &vars);
        assert_eq!(result, "FloatProperty:2.0");
    }

    #[test]
    fn test_multiple_vars() {
        let vars = [("min".into(), "10".into()), ("max".into(), "100".into())].into();
        let result = substitute("IntProperty:+{{ min }}({{ max }})", &vars);
        assert_eq!(result, "IntProperty:+10(100)");
    }

    #[test]
    fn test_unknown_var_unchanged() {
        let vars = HashMap::new();
        let result = substitute("{{ unknown }}", &vars);
        assert_eq!(result, "{{ unknown }}");
    }

    #[test]
    fn test_namespaced_var() {
        let vars = [("aircraftName".into(), "ACG-01X".into())].into();
        let result = substitute("*:'{{ vars.aircraftName }}'", &vars);
        assert_eq!(result, "*:'ACG-01X'");
    }

    #[test]
    fn test_no_template() {
        let vars = [("x".into(), "y".into())].into();
        let result = substitute("plain text", &vars);
        assert_eq!(result, "plain text");
    }

    #[test]
    fn test_apply_to_mod() {
        let json = r#"{
            "_id": "test",
            "_vars": { "mult": "2.0" },
            "assetPatches": {
                "test.uasset": [{
                    "name": "test",
                    "patches": [{
                        "template": "datatable:[*]",
                        "value": "FloatProperty:{{ mult }}",
                        "type": "propertyValue"
                    }]
                }]
            },
            "filePatches": {}
        }"#;
        let mut m = crate::manifest::parse_mod_json(json).unwrap();
        apply_variables_to_mod(&mut m);
        let patch = &m.asset_patches["test.uasset"][0].patches[0];
        assert_eq!(patch.value, "FloatProperty:2.0");
    }
}

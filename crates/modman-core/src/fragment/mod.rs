//! Fragment DSL parser — faithful port of the C# SicarioPatch template grammar
//! (`SicarioPatch.Engine/TemplateParser.cs` + `Templates/*TemplateProvider.cs`).
//!
//! Grammar: `loader(param)? ':' fragment ('.' fragment)*`
//!
//! Fragment alternatives (priority order mirrors the C# parser chain — the
//! struct providers are registered last and thus tried first, and within each
//! provider the parsers are tried in reverse registration order):
//!
//! | syntax              | fragment                                        |
//! |---------------------|-------------------------------------------------|
//! | `{name}` / `{name*}`| `StructProperty` — descend into struct children |
//! | `{Type:{Name=Val}}` | `StructMatch` — struct by type + child value    |
//! | `['name']` / `['!name']` | `StructName` — name match (`*` partial, `!` invert) |
//! | `<Type=A|B|C>`      | `NumberCollection` — numeric value set          |
//! | `<Type=Value>`      | `PropertyValue` — value constraint (`*` wildcard) |
//! | `<Type>`            | `PropertyType` — type filter                    |
//! | `<Enum::Member>`    | `EnumValue` — byte-property enum member         |
//! | `[[*]]`             | `ArrayFlatten` — flatten array items            |
//! | `[[N]]`             | `ArrayPropertyIndex` — index into array items   |
//! | `[N]`               | `ArrayIndex` — index into current set           |
//! | `[*]`               | `Any` — pass-through                            |
//! | `{**}`              | `Flatten` — descend into all structs            |

/// The result of parsing a template string
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateContext {
    /// The type loader to use (e.g., "datatable", "raw")
    pub loader: String,
    /// Optional parameter passed to the loader
    pub loader_param: Option<String>,
    /// The chain of fragments to apply
    pub fragments: Vec<Fragment>,
}

/// A single fragment filter in the chain
#[derive(Debug, Clone, PartialEq)]
pub enum Fragment {
    /// `[*]` — match everything (pass-through)
    Any,
    /// `{**}` — descend into all structs' children
    Flatten,
    /// `['name']`, `['name*']`, `['!name']` — match properties by name.
    /// `name` is stored without trailing `*`; `partial` enables prefix match.
    StructName {
        name: String,
        invert: bool,
        partial: bool,
    },
    /// `{name}`, `{name*}` — descend into struct children matching name.
    StructProperty { name: String, partial: bool },
    /// `{Type:{Name=Value}}` — match structs by type and child property value
    StructMatch {
        struct_type: Option<String>,
        prop_name: String,
        prop_value: String,
    },
    /// `[N]` — select Nth result from current set
    ArrayIndex(usize),
    /// `[[N]]` — select Nth entry from ArrayProperty items
    ArrayPropertyIndex(usize),
    /// `[[*]]` — flatten ArrayProperty items into the current set
    ArrayFlatten,
    /// `<Type>` — filter by property type
    PropertyType(String),
    /// `<Type=Value>` — filter by type and value (`*` wildcard semantics)
    PropertyValue {
        prop_type: String,
        value: Option<String>,
    },
    /// `<Type=A|B|C>` — filter by type and numeric value set
    NumberCollection { prop_type: String, values: Vec<f64> },
    /// `<Enum::Member>` / `<Enum::>` — filter byte properties by enum member
    EnumValue {
        enum_type: String,
        value: Option<String>,
    },
}

/// Errors from parsing the fragment DSL
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unexpected end of input")]
    UnexpectedEnd,
    #[error("expected '{0}' at position {1}")]
    Expected(char, usize),
    #[error("expected a fragment at position {0}: `{1}`")]
    UnknownFragment(usize, String),
    #[error("expected a type loader prefix (e.g. `datatable:`)")]
    MissingLoader,
}

/// Parse a complete template string into its context and fragment chain.
pub fn parse_template(input: &str) -> Result<TemplateContext, ParseError> {
    let mut p = Parser::new(input);

    // Type loader: identifier, optionally with a `(param)` argument.
    let loader = p.identifier().ok_or(ParseError::MissingLoader)?;
    p.skip_ws();
    let mut loader_param = None;
    if p.eat('(') {
        p.skip_ws();
        let param = p
            .quoted()
            .or_else(|| p.identifier())
            .ok_or(ParseError::Expected(')', p.pos))?;
        p.skip_ws();
        p.expect(')')?;
        loader_param = Some(param);
    }
    p.skip_ws();
    p.expect(':')?;

    let mut fragments = Vec::new();
    loop {
        let fragment = p
            .fragment()
            .ok_or_else(|| ParseError::UnknownFragment(p.pos, p.snippet()))?;
        fragments.push(fragment);
        p.skip_ws();
        if !p.eat('.') {
            break;
        }
    }
    p.skip_ws();
    if p.pos < p.chars.len() {
        return Err(ParseError::UnknownFragment(p.pos, p.snippet()));
    }
    Ok(TemplateContext {
        loader,
        loader_param,
        fragments,
    })
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

type FragmentTry = fn(&mut Parser) -> Option<Fragment>;

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), ParseError> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(ParseError::Expected(c, self.pos))
        }
    }

    /// Whitespace skip (Parlot `Terms.*` terminals skip whitespace).
    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn snippet(&self) -> String {
        let start = self.pos.saturating_sub(20);
        let end = (self.pos + 20).min(self.chars.len());
        self.chars[start..end].iter().collect()
    }

    fn identifier(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        match self.peek() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                self.pos += 1;
            }
            _ => return None,
        }
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            self.pos += 1;
        }
        Some(self.chars[start..self.pos].iter().collect())
    }

    /// Quoted string (`'...'` or `"..."`, no escape handling — matching the
    /// template corpus).
    fn quoted(&mut self) -> Option<String> {
        self.skip_ws();
        let quote = match self.peek() {
            Some(c @ ('\'' | '"')) => c,
            _ => return None,
        };
        self.pos += 1;
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == quote {
                let s: String = self.chars[start..self.pos].iter().collect();
                self.pos += 1;
                return Some(s);
            }
            self.pos += 1;
        }
        None
    }

    fn integer(&mut self) -> Option<i64> {
        self.skip_ws();
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        let digits_start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.pos == digits_start {
            self.pos = start;
            return None;
        }
        self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse()
            .ok()
    }

    fn number(&mut self) -> Option<f64> {
        self.skip_ws();
        let int_part = self.integer()?;
        let mut text = int_part.to_string();
        if self.peek() == Some('.') {
            let save = self.pos;
            self.pos += 1;
            let digits_start = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
            if self.pos == digits_start {
                self.pos = save;
            } else {
                text.push('.');
                for c in &self.chars[digits_start..self.pos] {
                    text.push(*c);
                }
            }
        }
        text.parse().ok()
    }

    /// Parse one fragment, trying alternatives in the C# priority order.
    fn fragment(&mut self) -> Option<Fragment> {
        const ATTEMPTS: &[FragmentTry] = &[
            try_struct_property,
            try_struct_match,
            try_struct_name,
            try_number_collection,
            try_property_value,
            try_property_type,
            try_enum_value,
            try_array_flatten,
            try_array_prop_index,
            try_array_index,
            try_defaults,
        ];
        let start = self.pos;
        for attempt in ATTEMPTS {
            self.pos = start;
            if let Some(f) = attempt(self) {
                return Some(f);
            }
        }
        self.pos = start;
        None
    }
}

/// `{name}` / `{name*}` → StructProperty
fn try_struct_property(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('{') {
        return None;
    }
    let name = p.quoted()?;
    p.skip_ws();
    if !p.eat('}') {
        return None;
    }
    let partial = name.ends_with('*');
    Some(Fragment::StructProperty {
        name: name.trim_end_matches('*').to_string(),
        partial,
    })
}

/// `{Type:{Name=Value}}` / `{{Name=Value}}` → StructMatch
fn try_struct_match(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('{') {
        return None;
    }
    let struct_type = p.identifier();
    p.skip_ws();
    if struct_type.is_some() && !p.eat(':') {
        return None;
    }
    if !p.eat('{') {
        return None;
    }
    let prop_name = p.quoted()?;
    p.skip_ws();
    if !p.eat('=') {
        return None;
    }
    let prop_value = p.quoted()?;
    p.skip_ws();
    if !p.eat('}') {
        return None;
    }
    p.skip_ws();
    if !p.eat('}') {
        return None;
    }
    Some(Fragment::StructMatch {
        struct_type,
        prop_name,
        prop_value,
    })
}

/// `['name']` / `['!name']` → StructName
fn try_struct_name(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('[') {
        return None;
    }
    p.skip_ws();
    let invert = p.eat('!');
    p.skip_ws();
    let name = p.quoted()?;
    p.skip_ws();
    if !p.eat(']') {
        return None;
    }
    let partial = name.ends_with('*');
    Some(Fragment::StructName {
        name: name.trim_end_matches('*').to_string(),
        invert,
        partial,
    })
}

/// `<Type=A|B|C>` → NumberCollection (tried before `<Type=Value>`: single
/// numeric values also land here, mirroring the C# parser priority).
fn try_number_collection(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('<') {
        return None;
    }
    let prop_type = p.identifier()?;
    p.skip_ws();
    if !p.eat('=') {
        return None;
    }
    let first = p.number()?;
    let mut values = vec![first];
    loop {
        p.skip_ws();
        if !p.eat('|') {
            break;
        }
        values.push(p.number()?);
    }
    p.skip_ws();
    if !p.eat('>') {
        return None;
    }
    Some(Fragment::NumberCollection { prop_type, values })
}

/// `<Type=Value>` (quoted string, number, or `*`) → PropertyValue
fn try_property_value(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('<') {
        return None;
    }
    let prop_type = p.identifier()?;
    p.skip_ws();
    if !p.eat('=') {
        return None;
    }
    p.skip_ws();
    let value = if p.eat('*') {
        "*".to_string()
    } else if let Some(s) = p.quoted() {
        s
    } else {
        let n = p.number()?;
        format_number(n)
    };
    p.skip_ws();
    if !p.eat('>') {
        return None;
    }
    Some(Fragment::PropertyValue {
        prop_type,
        value: Some(value),
    })
}

/// `<Type>` → PropertyType
fn try_property_type(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('<') {
        return None;
    }
    let prop_type = p.identifier()?;
    p.skip_ws();
    if !p.eat('>') {
        return None;
    }
    Some(Fragment::PropertyType(prop_type))
}

/// `<Enum::Member>` / `<Enum::>` → EnumValue
fn try_enum_value(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('<') {
        return None;
    }
    let enum_type = p.identifier()?;
    p.skip_ws();
    if !p.eat(':') || !p.eat(':') {
        return None;
    }
    p.skip_ws();
    let value = p.identifier();
    p.skip_ws();
    if !p.eat('>') {
        return None;
    }
    Some(Fragment::EnumValue { enum_type, value })
}

/// `[[*]]` → ArrayFlatten
fn try_array_flatten(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('[') || !p.eat('[') {
        return None;
    }
    p.skip_ws();
    if !p.eat('*') {
        return None;
    }
    p.skip_ws();
    if !p.eat(']') || !p.eat(']') {
        return None;
    }
    Some(Fragment::ArrayFlatten)
}

/// `[[N]]` → ArrayPropertyIndex
fn try_array_prop_index(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('[') || !p.eat('[') {
        return None;
    }
    let n = p.integer()?;
    p.skip_ws();
    if !p.eat(']') || !p.eat(']') {
        return None;
    }
    if n < 0 {
        return None;
    }
    Some(Fragment::ArrayPropertyIndex(n as usize))
}

/// `[N]` → ArrayIndex
fn try_array_index(p: &mut Parser) -> Option<Fragment> {
    if !p.eat('[') {
        return None;
    }
    let n = p.integer()?;
    p.skip_ws();
    if !p.eat(']') {
        return None;
    }
    if n < 0 {
        return None;
    }
    Some(Fragment::ArrayIndex(n as usize))
}

/// `[*]` → Any; `{**}` → Flatten
fn try_defaults(p: &mut Parser) -> Option<Fragment> {
    if p.eat('[') {
        p.skip_ws();
        if !p.eat('*') {
            return None;
        }
        p.skip_ws();
        if !p.eat(']') {
            return None;
        }
        return Some(Fragment::Any);
    }
    if p.eat('{') {
        p.skip_ws();
        if !p.eat('*') {
            return None;
        }
        p.skip_ws();
        if !p.eat('*') {
            return None;
        }
        p.skip_ws();
        if !p.eat('}') {
            return None;
        }
        return Some(Fragment::Flatten);
    }
    None
}

/// Format a parsed number the way it would appear in a template literal.
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frags(t: &str) -> Vec<Fragment> {
        parse_template(t).unwrap().fragments
    }

    #[test]
    fn parse_spear_fixed_loadout() {
        let ctx = parse_template("datatable:['SPEAR'].{'FixedLoadout*'}").unwrap();
        assert_eq!(ctx.loader, "datatable");
        assert_eq!(ctx.loader_param, None);
        assert_eq!(
            ctx.fragments,
            vec![
                Fragment::StructName {
                    name: "SPEAR".into(),
                    invert: false,
                    partial: false
                },
                Fragment::StructProperty {
                    name: "FixedLoadout".into(),
                    partial: true
                },
            ]
        );
    }

    #[test]
    fn parse_spear_hardpoint() {
        assert_eq!(
            frags("datatable:['SPEAR'].{'HardpointCompatibilityList*'}.[[3]].<StrProperty='rgps'>"),
            vec![
                Fragment::StructName {
                    name: "SPEAR".into(),
                    invert: false,
                    partial: false
                },
                Fragment::StructProperty {
                    name: "HardpointCompatibilityList".into(),
                    partial: true
                },
                Fragment::ArrayPropertyIndex(3),
                Fragment::PropertyValue {
                    prop_type: "StrProperty".into(),
                    value: Some("rgps".into())
                },
            ]
        );
    }

    #[test]
    fn parse_chimera_stats() {
        assert_eq!(
            frags("datatable:['ACG-01X'].[0].{'BaseStats*'}.{'MaxSpeed*'}.<FloatProperty='2500'>"),
            vec![
                Fragment::StructName {
                    name: "ACG-01X".into(),
                    invert: false,
                    partial: false
                },
                Fragment::ArrayIndex(0),
                Fragment::StructProperty {
                    name: "BaseStats".into(),
                    partial: true
                },
                Fragment::StructProperty {
                    name: "MaxSpeed".into(),
                    partial: true
                },
                Fragment::PropertyValue {
                    prop_type: "FloatProperty".into(),
                    value: Some("2500".into())
                },
            ]
        );
    }

    #[test]
    fn parse_any() {
        assert_eq!(frags("datatable:[*]"), vec![Fragment::Any]);
    }

    #[test]
    fn parse_array_flatten_chain() {
        assert_eq!(
            frags("datatable:{'HardpointCompatibilityList*'}.[[*]]"),
            vec![
                Fragment::StructProperty {
                    name: "HardpointCompatibilityList".into(),
                    partial: true
                },
                Fragment::ArrayFlatten,
            ]
        );
    }

    #[test]
    fn parse_enum_empty_member() {
        assert_eq!(
            frags("datatable:['RG-21'].{'BaseStats*'}.{'CannonType*'}.<S_CannonType::>"),
            vec![
                Fragment::StructName {
                    name: "RG-21".into(),
                    invert: false,
                    partial: false
                },
                Fragment::StructProperty {
                    name: "BaseStats".into(),
                    partial: true
                },
                Fragment::StructProperty {
                    name: "CannonType".into(),
                    partial: true
                },
                Fragment::EnumValue {
                    enum_type: "S_CannonType".into(),
                    value: None
                },
            ]
        );
    }

    #[test]
    fn parse_enum_member() {
        assert_eq!(
            frags("datatable:['Credits'].{'ButtonType*'}.<ButtonType::NewEnumerator1>"),
            vec![
                Fragment::StructName {
                    name: "Credits".into(),
                    invert: false,
                    partial: false
                },
                Fragment::StructProperty {
                    name: "ButtonType".into(),
                    partial: true
                },
                Fragment::EnumValue {
                    enum_type: "ButtonType".into(),
                    value: Some("NewEnumerator1".into())
                },
            ]
        );
    }

    #[test]
    fn parse_numeric_collection() {
        assert_eq!(
            frags("datatable:{'MissionCompletionBonus*'}.<IntProperty=2000>"),
            vec![
                Fragment::StructProperty {
                    name: "MissionCompletionBonus".into(),
                    partial: true
                },
                Fragment::NumberCollection {
                    prop_type: "IntProperty".into(),
                    values: vec![2000.0]
                },
            ]
        );
        assert_eq!(
            frags("datatable:<FloatProperty=1|2.5|300>"),
            vec![Fragment::NumberCollection {
                prop_type: "FloatProperty".into(),
                values: vec![1.0, 2.5, 300.0]
            }]
        );
    }

    #[test]
    fn parse_quoted_numeric_is_property_value() {
        // Quoted numbers stay PropertyValueFragment (string compare).
        assert_eq!(
            frags("datatable:<FloatProperty='2'>"),
            vec![Fragment::PropertyValue {
                prop_type: "FloatProperty".into(),
                value: Some("2".into())
            }]
        );
    }

    #[test]
    fn parse_type_only() {
        assert_eq!(
            frags("datatable:['RailgunPodSlow'].{'ReloadTime*'}.<FloatProperty>"),
            vec![
                Fragment::StructName {
                    name: "RailgunPodSlow".into(),
                    invert: false,
                    partial: false
                },
                Fragment::StructProperty {
                    name: "ReloadTime".into(),
                    partial: true
                },
                Fragment::PropertyType("FloatProperty".into()),
            ]
        );
    }

    #[test]
    fn parse_invert() {
        // C# grammar: `ZeroOrOne('!').And(String())` — the bang precedes the quoted name.
        assert_eq!(
            frags("datatable:[!'F-15C']"),
            vec![Fragment::StructName {
                name: "F-15C".into(),
                invert: true,
                partial: false
            }]
        );
    }

    #[test]
    fn parse_flatten_all() {
        assert_eq!(frags("datatable:{**}"), vec![Fragment::Flatten]);
    }

    #[test]
    fn parse_struct_match() {
        // C# grammar: `{Type?:{'Name'='Value'}}` — strings are quoted.
        assert_eq!(
            frags("datatable:{SSchemeStruct:{'SchemeIndex'='2'}}"),
            vec![Fragment::StructMatch {
                struct_type: Some("SSchemeStruct".into()),
                prop_name: "SchemeIndex".into(),
                prop_value: "2".into()
            }]
        );
        assert_eq!(
            frags("datatable:{{'Name'='Value'}}"),
            vec![Fragment::StructMatch {
                struct_type: None,
                prop_name: "Name".into(),
                prop_value: "Value".into()
            }]
        );
    }

    #[test]
    fn parse_wildcard_value() {
        assert_eq!(
            frags("datatable:{'IsAvailable*'}.<BoolProperty=*>"),
            vec![
                Fragment::StructProperty {
                    name: "IsAvailable".into(),
                    partial: true
                },
                Fragment::PropertyValue {
                    prop_type: "BoolProperty".into(),
                    value: Some("*".into())
                },
            ]
        );
    }

    #[test]
    fn parse_loader_param() {
        let ctx = parse_template("datatable(TableName):['a']").unwrap();
        assert_eq!(ctx.loader_param.as_deref(), Some("TableName"));
        let ctx = parse_template("raw('param value'):['a']").unwrap();
        assert_eq!(ctx.loader_param.as_deref(), Some("param value"));
    }

    #[test]
    fn parse_errors() {
        assert!(parse_template("").is_err());
        assert!(parse_template("datatable:").is_err());
        assert!(parse_template("['a']").is_err()); // no loader
        assert!(parse_template("datatable:['a']garbage").is_err());
        assert!(parse_template("datatable:[nope]").is_err());
    }

    #[test]
    fn double_quoted_strings_accepted() {
        assert_eq!(
            frags("datatable:[\"SPEAR\"]"),
            vec![Fragment::StructName {
                name: "SPEAR".into(),
                invert: false,
                partial: false
            }]
        );
    }
}

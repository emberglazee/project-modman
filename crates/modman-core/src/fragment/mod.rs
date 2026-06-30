//! Fragment DSL parser — translates Sicario template strings into executable fragments.
//!
//! Grammar:
//!   template = type_loader ":" fragment_chain
//!   type_loader = identifier ("(" parameter? ")")?
//!   fragment_chain = fragment ("." fragment)*

/// The result of parsing a template string
#[derive(Debug, Clone)]
pub struct TemplateContext {
    /// The type loader to use (e.g., "datatable", "raw")
    pub loader: String,
    /// Optional parameter passed to the loader (e.g., table name)
    pub loader_param: Option<String>,
    /// The chain of fragments to apply
    pub fragments: Vec<Fragment>,
}

/// A single fragment filter in the chain
#[derive(Debug, Clone, PartialEq)]
pub enum Fragment {
    /// `['name']` — match properties by name (supports `*` wildcard, `!` invert)
    StructName { name: String, invert: bool },
    /// `[N]` — select Nth result from current set
    ArrayIndex(usize),
    /// `{name}` — descend into struct, return children matching name
    StructProperty(String),
    /// `{Type:{Name=Value}}` — descend into struct with child property match
    StructMatch {
        struct_type: Option<String>,
        prop_name: String,
        prop_value: String,
    },
    /// `[[N]]` — select Nth entry from ArrayProperty
    ArrayPropertyIndex(usize),
    /// `[[*]]` — flatten array contents
    ArrayFlatten,
    /// `<type>` — filter by property type
    PropertyType(String),
    /// `<type=value>` — filter by type and value
    PropertyValue { prop_type: String, value: String },
    /// `<type::value>` — filter enum by type and value
    EnumValue {
        enum_type: String,
        enum_value: String,
    },
    /// `[*]` — match everything
    Any,
    /// `{**}` — flatten nested structures
    Flatten,
}

/// Errors from parsing the fragment DSL
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Unexpected end of input")]
    UnexpectedEnd,
    #[error("Expected '{0}' at position {1}")]
    Expected(char, usize),
    #[error("Unknown fragment syntax at position {0}: `{1}`")]
    UnknownSyntax(usize, String),
    #[error("Invalid type loader at position {0}: `{1}`")]
    InvalidLoader(usize, String),
}

/// Parse a complete template string into a TemplateContext
pub fn parse_template(input: &str) -> Result<TemplateContext, ParseError> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;

    // Parse type loader: identifier ("(" param? ")")?
    let loader = parse_identifier(&chars, &mut pos)?;
    let loader_param = if pos < chars.len() && chars[pos] == '(' {
        pos += 1; // skip '('
        let param = parse_string_or_ident(&chars, &mut pos)?;
        expect_char(&chars, &mut pos, ')')?;
        Some(param)
    } else {
        None
    };

    // Expect ':'
    expect_char(&chars, &mut pos, ':')?;

    // Parse fragment chain
    let mut fragments = Vec::new();
    if pos < chars.len() {
        fragments.push(parse_fragment(&chars, &mut pos)?);
        while pos < chars.len() && chars[pos] == '.' {
            pos += 1; // skip '.'
            if pos < chars.len() {
                fragments.push(parse_fragment(&chars, &mut pos)?);
            }
        }
    }

    Ok(TemplateContext {
        loader: loader.to_lowercase(),
        loader_param,
        fragments,
    })
}

fn parse_fragment(chars: &[char], pos: &mut usize) -> Result<Fragment, ParseError> {
    if *pos >= chars.len() {
        return Err(ParseError::UnexpectedEnd);
    }

    match chars[*pos] {
        '[' => {
            *pos += 1;
            if *pos >= chars.len() {
                return Err(ParseError::UnexpectedEnd);
            }
            if chars[*pos] == '[' {
                // [[N]] or [[*]]
                *pos += 1;
                if *pos < chars.len() && chars[*pos] == '*' {
                    *pos += 1;
                    expect_char(chars, pos, ']')?;
                    expect_char(chars, pos, ']')?;
                    Ok(Fragment::ArrayFlatten)
                } else {
                    let index = parse_number(chars, pos)?;
                    expect_char(chars, pos, ']')?;
                    expect_char(chars, pos, ']')?;
                    Ok(Fragment::ArrayPropertyIndex(index))
                }
            } else if chars[*pos] == '*' {
                // [*] — Any fragment
                *pos += 1;
                expect_char(chars, pos, ']')?;
                Ok(Fragment::Any)
            } else if chars[*pos] == '!' {
                // [!name] — Inverted struct fragment
                *pos += 1;
                let name = parse_string_or_ident(chars, pos)?;
                expect_char(chars, pos, ']')?;
                Ok(Fragment::StructName { name, invert: true })
            } else {
                // [name] or [N]
                let val = parse_string_or_ident(chars, pos)?;
                expect_char(chars, pos, ']')?;
                // Check if it's a number
                if let Ok(n) = val.parse::<usize>() {
                    Ok(Fragment::ArrayIndex(n))
                } else {
                    Ok(Fragment::StructName {
                        name: val,
                        invert: false,
                    })
                }
            }
        }
        '{' => {
            *pos += 1;
            if *pos < chars.len()
                && *pos + 1 < chars.len()
                && chars[*pos] == '*'
                && chars[*pos + 1] == '*'
            {
                // {**} — Flatten
                *pos += 2;
                expect_char(chars, pos, '}')?;
                Ok(Fragment::Flatten)
            } else if *pos < chars.len() && chars[*pos] == '*' {
                // {*:{Name=Value}} — StructMatch with wildcard type
                *pos += 1;
                expect_char(chars, pos, ':')?;
                expect_char(chars, pos, '{')?;
                let prop_name = parse_string_or_ident(chars, pos)?;
                expect_char(chars, pos, '=')?;
                let prop_value = parse_string_or_ident(chars, pos)?;
                expect_char(chars, pos, '}')?;
                expect_char(chars, pos, '}')?;
                Ok(Fragment::StructMatch {
                    struct_type: None,
                    prop_name,
                    prop_value,
                })
            } else if *pos < chars.len() && chars[*pos] == ':' {
                // {:? actually {Type:{...}}
                // Find the opening { after the type
                let struct_type = Some(parse_identifier(chars, pos)?);
                expect_char(chars, pos, ':')?;
                expect_char(chars, pos, '{')?;
                let prop_name = parse_string_or_ident(chars, pos)?;
                expect_char(chars, pos, '=')?;
                let prop_value = parse_string_or_ident(chars, pos)?;
                expect_char(chars, pos, '}')?;
                expect_char(chars, pos, '}')?;
                Ok(Fragment::StructMatch {
                    struct_type,
                    prop_name,
                    prop_value,
                })
            } else {
                // {name} — StructPropertyFragment
                let name = parse_string_or_ident(chars, pos)?;
                expect_char(chars, pos, '}')?;
                Ok(Fragment::StructProperty(name))
            }
        }
        '<' => {
            *pos += 1;
            let content = parse_until(chars, pos, '>')?;
            expect_char(chars, pos, '>')?;

            // <type::enum_value> — EnumValue
            if let Some(double_colon) = content.find("::") {
                let enum_type = content[..double_colon].to_string();
                let enum_value = content[double_colon + 2..].to_string();
                Ok(Fragment::EnumValue {
                    enum_type,
                    enum_value,
                })
            }
            // <type=value> — PropertyValue
            else if let Some(eq_pos) = content.find('=') {
                let prop_type = content[..eq_pos].to_string();
                let value = content[eq_pos + 1..].to_string();
                // Strip single quotes from value
                let value = value.trim_matches('\'').to_string();
                Ok(Fragment::PropertyValue { prop_type, value })
            }
            // <type> — PropertyType
            else {
                Ok(Fragment::PropertyType(content.trim().to_string()))
            }
        }
        _ => Err(ParseError::UnknownSyntax(*pos, chars.iter().collect())),
    }
}

// ---- Parser helpers ----

fn parse_identifier(chars: &[char], pos: &mut usize) -> Result<String, ParseError> {
    let start = *pos;
    while *pos < chars.len() && (chars[*pos].is_alphanumeric() || chars[*pos] == '_') {
        *pos += 1;
    }
    if *pos == start {
        return Err(ParseError::InvalidLoader(*pos, chars.iter().collect()));
    }
    Ok(chars[start..*pos].iter().collect())
}

fn parse_string_or_ident(chars: &[char], pos: &mut usize) -> Result<String, ParseError> {
    if *pos >= chars.len() {
        return Err(ParseError::UnexpectedEnd);
    }
    if chars[*pos] == '\'' {
        *pos += 1;
        let start = *pos;
        while *pos < chars.len() && chars[*pos] != '\'' {
            *pos += 1;
        }
        if *pos >= chars.len() {
            return Err(ParseError::UnexpectedEnd);
        }
        let result: String = chars[start..*pos].iter().collect();
        *pos += 1; // skip closing '
        Ok(result)
    } else {
        parse_identifier(chars, pos)
    }
}

fn parse_number(chars: &[char], pos: &mut usize) -> Result<usize, ParseError> {
    let start = *pos;
    while *pos < chars.len() && chars[*pos].is_ascii_digit() {
        *pos += 1;
    }
    if *pos == start {
        return Err(ParseError::Expected('0', *pos));
    }
    let s: String = chars[start..*pos].iter().collect();
    s.parse::<usize>()
        .map_err(|_| ParseError::Expected('0', start))
}

fn parse_until(chars: &[char], pos: &mut usize, delim: char) -> Result<String, ParseError> {
    let start = *pos;
    while *pos < chars.len() && chars[*pos] != delim {
        *pos += 1;
    }
    if *pos >= chars.len() {
        return Err(ParseError::Expected(delim, *pos));
    }
    Ok(chars[start..*pos].iter().collect())
}

fn expect_char(chars: &[char], pos: &mut usize, expected: char) -> Result<(), ParseError> {
    if *pos >= chars.len() {
        return Err(ParseError::Expected(expected, *pos));
    }
    if chars[*pos] != expected {
        return Err(ParseError::Expected(expected, *pos));
    }
    *pos += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_datatable_simple() {
        let ctx = parse_template("datatable:{'BaseStats*'}.{'CanUseAoA*'}").unwrap();
        assert_eq!(ctx.loader, "datatable");
        assert_eq!(ctx.loader_param, None);
        assert_eq!(ctx.fragments.len(), 2);
        assert_eq!(
            ctx.fragments[0],
            Fragment::StructProperty("BaseStats*".to_string())
        );
        assert_eq!(
            ctx.fragments[1],
            Fragment::StructProperty("CanUseAoA*".to_string())
        );
    }

    #[test]
    fn parse_full_example() {
        let ctx =
            parse_template("datatable:['F-15C'].[0].{'HardpointSlots*'}.[[1]].<IntProperty='2'>")
                .unwrap();
        assert_eq!(ctx.loader, "datatable");
        assert_eq!(ctx.fragments.len(), 5);
        assert_eq!(
            ctx.fragments[0],
            Fragment::StructName {
                name: "F-15C".into(),
                invert: false
            }
        );
        assert_eq!(ctx.fragments[1], Fragment::ArrayIndex(0));
        assert_eq!(
            ctx.fragments[2],
            Fragment::StructProperty("HardpointSlots*".into())
        );
        assert_eq!(ctx.fragments[3], Fragment::ArrayPropertyIndex(1));
        assert_eq!(
            ctx.fragments[4],
            Fragment::PropertyValue {
                prop_type: "IntProperty".into(),
                value: "2".into()
            }
        );
    }

    #[test]
    fn parse_enum() {
        let ctx = parse_template("datatable:<S_CannonType::NewEnumerator2>").unwrap();
        assert_eq!(ctx.fragments.len(), 1);
        assert_eq!(
            ctx.fragments[0],
            Fragment::EnumValue {
                enum_type: "S_CannonType".into(),
                enum_value: "NewEnumerator2".into()
            }
        );
    }

    #[test]
    fn parse_any() {
        let ctx = parse_template("datatable:[*]").unwrap();
        assert_eq!(ctx.fragments.len(), 1);
        assert_eq!(ctx.fragments[0], Fragment::Any);
    }

    #[test]
    fn parse_array_flatten() {
        let ctx = parse_template("datatable:[[*]]").unwrap();
        assert_eq!(ctx.fragments.len(), 1);
        assert_eq!(ctx.fragments[0], Fragment::ArrayFlatten);
    }

    #[test]
    fn parse_struct_match() {
        let ctx = parse_template("datatable:{*:{'Subtitle*'='0_Subtitle*'}}").unwrap();
        assert_eq!(ctx.fragments.len(), 1);
        assert_eq!(
            ctx.fragments[0],
            Fragment::StructMatch {
                struct_type: None,
                prop_name: "Subtitle*".into(),
                prop_value: "0_Subtitle*".into()
            }
        );
    }

    #[test]
    fn parse_with_param() {
        let ctx = parse_template("datatable(MyTable):[*]").unwrap();
        assert_eq!(ctx.loader, "datatable");
        assert_eq!(ctx.loader_param, Some("MyTable".into()));
        assert_eq!(ctx.fragments.len(), 1);
        assert_eq!(ctx.fragments[0], Fragment::Any);
    }
}

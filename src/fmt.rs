/// Canonical source formatting that deliberately operates on source text rather
/// than the AST so comments and the original spelling of byte strings survive.
///
/// The v0.1 canonical Unicode spelling is `≈`; custom operators are already
/// written as Unicode glyphs in source. Layout is normalized without moving
/// tokens between lines, which keeps the formatter safe around record literals
/// and expression blocks that both use braces.
pub fn format_source(source: &str) -> String {
    let mut output = String::new();
    let mut indent = 0usize;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

    for raw_line in source.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            if !output.ends_with("\n\n") {
                output.push('\n');
            }
            continue;
        }

        let starts_with_close = trimmed.starts_with('}');
        if starts_with_close {
            indent = indent.saturating_sub(1);
        }

        let mut canonical = String::with_capacity(trimmed.len());
        let chars: Vec<char> = trimmed.chars().collect();
        let mut i = 0usize;
        let mut opens = 0usize;
        let mut closes = usize::from(starts_with_close);
        while i < chars.len() {
            let c = chars[i];
            if !in_string && !in_char && c == '/' && chars.get(i + 1) == Some(&'/') {
                canonical.extend(chars[i..].iter());
                break;
            }
            if !in_string && !in_char && c == '~' && chars.get(i + 1) == Some(&'=') {
                canonical.push('≈');
                i += 2;
                continue;
            }
            canonical.push(c);
            if in_string || in_char {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if in_string && c == '"' {
                    in_string = false;
                } else if in_char && c == '\'' {
                    in_char = false;
                }
            } else {
                match c {
                    '"' => in_string = true,
                    '\'' => in_char = true,
                    '{' => opens += 1,
                    '}' => closes += 1,
                    _ => {}
                }
            }
            i += 1;
        }

        output.push_str(&"  ".repeat(indent));
        output.push_str(canonicalize_operator_calls(canonical.trim_end()).as_str());
        output.push('\n');
        indent = indent.saturating_add(opens).saturating_sub(closes);
    }

    while output.starts_with('\n') {
        output.remove(0);
    }
    while output.ends_with("\n\n") {
        output.pop();
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn decode_operator_name(name: &str) -> Option<Vec<char>> {
    let encoded = name.strip_prefix("operator_u")?;
    encoded
        .split("_u")
        .map(|hex| u32::from_str_radix(hex, 16).ok().and_then(char::from_u32))
        .collect()
}

fn canonicalize_operator_calls(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while i < chars.len() {
        if !in_string && chars[i] == '/' && chars.get(i + 1) == Some(&'/') {
            out.extend(chars[i..].iter());
            break;
        }
        if chars[i] == '"' {
            in_string = !in_string;
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if in_string {
            out.push(chars[i]);
            if escaped {
                escaped = false;
            } else if chars[i] == '\\' {
                escaped = true;
            }
            i += 1;
            continue;
        }
        let rest: String = chars[i..].iter().collect();
        if rest.starts_with("operator_u") && (i == 0 || !chars[i - 1].is_ascii_alphanumeric()) {
            let mut end = i;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let name: String = chars[i..end].iter().collect();
            if let (Some(glyphs), Some('(')) = (decode_operator_name(&name), chars.get(end)) {
                let mut close = end + 1;
                let mut depth = 1usize;
                let mut quoted = false;
                let mut quote_escape = false;
                while close < chars.len() && depth > 0 {
                    let c = chars[close];
                    if quoted {
                        if quote_escape {
                            quote_escape = false;
                        } else if c == '\\' {
                            quote_escape = true;
                        } else if c == '"' {
                            quoted = false;
                        }
                    } else {
                        match c {
                            '"' => quoted = true,
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                    }
                    close += 1;
                }
                if depth == 0 {
                    let body: String = chars[end + 1..close - 1].iter().collect();
                    let args = split_call_args(&body);
                    if glyphs.len() == 1 && args.len() == 2 {
                        out.push('(');
                        out.push_str(canonicalize_operator_calls(args[0].trim()).as_str());
                        out.push(' ');
                        out.push(glyphs[0]);
                        out.push(' ');
                        out.push_str(canonicalize_operator_calls(args[1].trim()).as_str());
                        out.push(')');
                        i = close;
                        continue;
                    }
                    if glyphs.len() == 2 && args.len() == 1 {
                        out.push(glyphs[0]);
                        out.push_str(canonicalize_operator_calls(args[0].trim()).as_str());
                        out.push(glyphs[1]);
                        i = close;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn split_call_args(source: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, c) in source.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&source[start..index]);
                start = index + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&source[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_approximation_and_layout_without_touching_literals_or_comments() {
        let source = " main {\nprint(1.0 ~= 1.0)  \nprint(\"~=\") // ~= stays\n }\n";
        assert_eq!(
            format_source(source),
            "main {\n  print(1.0 ≈ 1.0)\n  print(\"~=\") // ~= stays\n}\n"
        );
    }

    #[test]
    fn formatting_is_idempotent() {
        let once = format_source("main {\nprint(1 ~= 1)\n}\n");
        assert_eq!(format_source(&once), once);
    }

    #[test]
    fn canonicalizes_ascii_operator_calls() {
        assert_eq!(
            format_source(
                "main {\n print(operator_u2295(1, operator_u2295(2, 3)))\n print(operator_u2016_u2016(4))\n}\n"
            ),
            "main {\n  print((1 ⊕ (2 ⊕ 3)))\n  print(‖4‖)\n}\n"
        );
    }
}

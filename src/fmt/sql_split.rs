//! Split a SQL blob on top-level `;` boundaries while honoring `'…'` / `"…"`
//! string literals, `[…]` and `` `…` `` quoted identifiers, and `--` / `/* */`
//! comments. Returns each statement (trailing `;` preserved).

pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_bracket = false;
    let mut in_backtick = false;

    while i < bytes.len() {
        let c = bytes[i] as char;

        if in_single {
            buf.push(c);
            if c == '\'' {
                if i + 1 < bytes.len() && bytes[i + 1] as char == '\'' {
                    buf.push('\'');
                    i += 2;
                    continue;
                }
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            buf.push(c);
            if c == '"' {
                if i + 1 < bytes.len() && bytes[i + 1] as char == '"' {
                    buf.push('"');
                    i += 2;
                    continue;
                }
                in_double = false;
            }
            i += 1;
            continue;
        }
        if in_bracket {
            buf.push(c);
            if c == ']' {
                in_bracket = false;
            }
            i += 1;
            continue;
        }
        if in_backtick {
            buf.push(c);
            if c == '`' {
                in_backtick = false;
            }
            i += 1;
            continue;
        }

        if c == '-' && i + 1 < bytes.len() && bytes[i + 1] as char == '-' {
            while i < bytes.len() && bytes[i] as char != '\n' {
                buf.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] as char == '*' {
            buf.push('/');
            buf.push('*');
            i += 2;
            while i + 1 < bytes.len()
                && !(bytes[i] as char == '*' && bytes[i + 1] as char == '/')
            {
                buf.push(bytes[i] as char);
                i += 1;
            }
            if i + 1 < bytes.len() {
                buf.push('*');
                buf.push('/');
                i += 2;
            }
            continue;
        }

        match c {
            '\'' => {
                in_single = true;
                buf.push(c);
            }
            '"' => {
                in_double = true;
                buf.push(c);
            }
            '[' => {
                in_bracket = true;
                buf.push(c);
            }
            '`' => {
                in_backtick = true;
                buf.push(c);
            }
            ';' => {
                buf.push(';');
                let trimmed = buf.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
                buf.clear();
            }
            _ => buf.push(c),
        }
        i += 1;
    }

    let trimmed = buf.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_two_simple_statements() {
        let v = split_statements("SELECT 1; SELECT 2;");
        assert_eq!(v, vec!["SELECT 1;", "SELECT 2;"]);
    }

    #[test]
    fn ignores_semicolons_in_strings() {
        let v = split_statements("INSERT INTO t VALUES ('a;b'); SELECT 1;");
        assert_eq!(v.len(), 2);
        assert!(v[0].contains("'a;b'"));
    }

    #[test]
    fn handles_doubled_quotes() {
        let v = split_statements("SELECT 'it''s'; SELECT 2;");
        assert_eq!(v.len(), 2);
        assert!(v[0].contains("'it''s'"));
    }

    #[test]
    fn skips_comments() {
        let v = split_statements("-- ;\nSELECT 1; /* ; */ SELECT 2;");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn no_trailing_semicolon_still_emits() {
        let v = split_statements("SELECT 1");
        assert_eq!(v, vec!["SELECT 1"]);
    }
}

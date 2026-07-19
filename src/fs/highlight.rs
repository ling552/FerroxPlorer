//! 轻量代码语法高亮：把源码拆成 4 个「字符对齐」的图层字符串。
//!
//! Slint 的 Text 不支持富文本，无法在单个元素内混排颜色。这里采用分层染色：
//! 每层保留本类别的字符、其它位置用等宽占位符（ASCII→空格、宽字符→全角空格）
//! 填充，UI 端用同一等宽字体把 4 层 Text 原位叠加，即可得到多色代码显示。
//! 层与层严格逐字符对齐，换行符在所有层同步出现。

/// 高亮结果：4 个等长图层（正文 / 关键字与数字 / 字符串 / 注释）
pub struct HighlightLayers {
    pub base: String,
    pub keywords: String,
    pub strings: String,
    pub comments: String,
}

/// 语言风格：注释语法 + 关键字表
struct Style {
    line_comment: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    keywords: &'static [&'static str],
}

const RUST_KW: &[&str] = &[
    "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "trait", "match", "if",
    "else", "for", "while", "loop", "return", "self", "Self", "super", "crate", "const", "static",
    "ref", "move", "async", "await", "dyn", "where", "type", "unsafe", "as", "in", "break",
    "continue", "true", "false", "None", "Some", "Ok", "Err",
];

const CLIKE_KW: &[&str] = &[
    "function",
    "var",
    "let",
    "const",
    "if",
    "else",
    "for",
    "while",
    "do",
    "switch",
    "case",
    "break",
    "continue",
    "return",
    "new",
    "class",
    "extends",
    "implements",
    "interface",
    "public",
    "private",
    "protected",
    "static",
    "void",
    "int",
    "float",
    "double",
    "bool",
    "boolean",
    "char",
    "long",
    "short",
    "unsigned",
    "signed",
    "struct",
    "enum",
    "union",
    "typedef",
    "import",
    "export",
    "from",
    "default",
    "try",
    "catch",
    "finally",
    "throw",
    "throws",
    "async",
    "await",
    "yield",
    "true",
    "false",
    "null",
    "undefined",
    "nullptr",
    "this",
    "super",
    "package",
    "namespace",
    "using",
    "template",
    "typename",
    "virtual",
    "override",
    "delete",
    "sizeof",
    "goto",
    "auto",
    "register",
    "volatile",
    "extern",
    "inline",
    "operator",
    "friend",
    "explicit",
    "constexpr",
    "decltype",
    "noexcept",
    "final",
    "abstract",
    "synchronized",
    "instanceof",
    "func",
    "go",
    "chan",
    "defer",
    "select",
    "map",
    "range",
    "fallthrough",
    "string",
    "byte",
    "rune",
    "uint",
    "int8",
    "int16",
    "int32",
    "int64",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "float32",
    "float64",
    "nil",
    "type",
    "readonly",
    "declare",
    "module",
    "any",
    "number",
    "unknown",
    "never",
    "symbol",
    "of",
    "in",
    "typeof",
    "keyof",
    "get",
    "set",
];

const PY_KW: &[&str] = &[
    "def", "class", "if", "elif", "else", "for", "while", "return", "import", "from", "as", "with",
    "try", "except", "finally", "raise", "lambda", "pass", "break", "continue", "global",
    "nonlocal", "yield", "async", "await", "True", "False", "None", "not", "and", "or", "in", "is",
    "del", "assert", "self", "match", "case",
];

const SH_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac", "function",
    "return", "local", "export", "echo", "exit", "set", "in", "read", "shift", "true", "false",
    "param", "foreach", "switch",
];

const SQL_KW: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "delete", "create", "table",
    "drop", "alter", "index", "join", "left", "right", "inner", "outer", "on", "group", "by",
    "order", "having", "limit", "offset", "as", "and", "or", "not", "null", "primary", "key",
    "foreign", "unique", "distinct", "union", "all", "exists", "between", "like", "SELECT", "FROM",
    "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "DELETE", "CREATE", "TABLE", "DROP", "ALTER",
    "INDEX", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "GROUP", "BY", "ORDER", "HAVING",
    "LIMIT", "AS", "AND", "OR", "NOT", "NULL", "PRIMARY", "KEY", "FOREIGN", "UNIQUE", "DISTINCT",
    "UNION", "ALL", "EXISTS", "BETWEEN", "LIKE",
];

const LUA_KW: &[&str] = &[
    "function", "local", "end", "if", "then", "else", "elseif", "for", "while", "do", "repeat",
    "until", "return", "break", "in", "pairs", "ipairs", "nil", "true", "false", "and", "or",
    "not",
];

const TOML_KW: &[&str] = &["true", "false"];

fn style_of(ext: &str) -> Option<Style> {
    let s = match ext {
        "rs" => Style {
            line_comment: &["//"],
            block_comment: Some(("/*", "*/")),
            keywords: RUST_KW,
        },
        "js" | "ts" | "jsx" | "tsx" | "c" | "h" | "cpp" | "hpp" | "cc" | "cs" | "java" | "kt"
        | "go" | "swift" | "php" | "scss" | "less" | "slint" | "vue" | "svelte" | "gradle" => {
            Style {
                line_comment: &["//"],
                block_comment: Some(("/*", "*/")),
                keywords: CLIKE_KW,
            }
        }
        "css" => Style {
            line_comment: &[],
            block_comment: Some(("/*", "*/")),
            keywords: &[],
        },
        "py" => Style {
            line_comment: &["#"],
            block_comment: None,
            keywords: PY_KW,
        },
        "sh" | "bat" | "ps1" | "yaml" | "yml" | "gitignore" | "dockerfile" | "properties"
        | "env" | "conf" | "cfg" | "ini" => Style {
            line_comment: &["#", "::", "rem ", "REM ", ";"],
            block_comment: None,
            keywords: SH_KW,
        },
        "toml" => Style {
            line_comment: &["#"],
            block_comment: None,
            keywords: TOML_KW,
        },
        "sql" => Style {
            line_comment: &["--"],
            block_comment: Some(("/*", "*/")),
            keywords: SQL_KW,
        },
        "lua" => Style {
            line_comment: &["--"],
            block_comment: None,
            keywords: LUA_KW,
        },
        "html" | "htm" | "xml" | "md" | "markdown" => Style {
            line_comment: &[],
            block_comment: Some(("<!--", "-->")),
            keywords: &[],
        },
        "json" => Style {
            line_comment: &[],
            block_comment: None,
            keywords: &["true", "false", "null"],
        },
        _ => return None,
    };
    Some(s)
}

/// 目标层
#[derive(Clone, Copy, PartialEq)]
enum Layer {
    Base,
    Keyword,
    Str,
    Comment,
}

/// 与字符等宽的占位符：宽字符（CJK/全角）用全角空格保持列对齐
fn pad_for(c: char) -> char {
    if c == '\t' {
        '\t'
    } else if (c as u32) >= 0x2E80 {
        '\u{3000}'
    } else {
        ' '
    }
}

/// 把源码按扩展名高亮为 4 个对齐图层。
/// 未识别的扩展名：全部进 base 层（纯文本显示）。
pub fn highlight(text: &str, ext: &str) -> HighlightLayers {
    let Some(style) = style_of(&ext.to_lowercase()) else {
        let blank = text
            .chars()
            .map(|c| if c == '\n' { '\n' } else { pad_for(c) })
            .collect::<String>();
        return HighlightLayers {
            base: text.to_string(),
            keywords: blank.clone(),
            strings: blank.clone(),
            comments: blank,
        };
    };

    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    // 每个字符所属层
    let mut layer = vec![Layer::Base; n];

    let starts_with_at = |i: usize, pat: &str| -> bool {
        pat.chars()
            .enumerate()
            .all(|(k, pc)| chars.get(i + k) == Some(&pc))
    };

    let mut i = 0;
    while i < n {
        let c = chars[i];
        // 行注释
        if let Some(lc) = style.line_comment.iter().find(|p| starts_with_at(i, p)) {
            let _ = lc;
            while i < n && chars[i] != '\n' {
                layer[i] = Layer::Comment;
                i += 1;
            }
            continue;
        }
        // 块注释
        if let Some((open, close)) = style.block_comment {
            if starts_with_at(i, open) {
                while i < n {
                    layer[i] = Layer::Comment;
                    if starts_with_at(i, close) {
                        // 标记结束符整体后跳出
                        for k in 0..close.chars().count() {
                            if i + k < n {
                                layer[i + k] = Layer::Comment;
                            }
                        }
                        i += close.chars().count();
                        break;
                    }
                    i += 1;
                }
                continue;
            }
        }
        // 字符串（" ' `；带反斜杠转义；不跨行——遇换行终止，容错未闭合情形）
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            layer[i] = Layer::Str;
            i += 1;
            while i < n && chars[i] != '\n' {
                layer[i] = Layer::Str;
                if chars[i] == '\\' && i + 1 < n && chars[i + 1] != '\n' {
                    layer[i + 1] = Layer::Str;
                    i += 2;
                    continue;
                }
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        // 数字（含 0x/小数；并入关键字层染同色）
        if c.is_ascii_digit() {
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                layer[i] = Layer::Keyword;
                i += 1;
            }
            continue;
        }
        // 标识符：整词取出后查关键字表
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if style.keywords.contains(&word.as_str()) {
                for slot in layer.iter_mut().take(i).skip(start) {
                    *slot = Layer::Keyword;
                }
            }
            continue;
        }
        i += 1;
    }

    // 按层展开为对齐字符串
    let mut base = String::with_capacity(n);
    let mut kw = String::with_capacity(n);
    let mut st = String::with_capacity(n);
    let mut cm = String::with_capacity(n);
    for (idx, &c) in chars.iter().enumerate() {
        if c == '\n' {
            base.push('\n');
            kw.push('\n');
            st.push('\n');
            cm.push('\n');
            continue;
        }
        if c == '\r' {
            continue;
        }
        let pad = pad_for(c);
        match layer[idx] {
            Layer::Base => {
                base.push(c);
                kw.push(pad);
                st.push(pad);
                cm.push(pad);
            }
            Layer::Keyword => {
                base.push(pad);
                kw.push(c);
                st.push(pad);
                cm.push(pad);
            }
            Layer::Str => {
                base.push(pad);
                kw.push(pad);
                st.push(c);
                cm.push(pad);
            }
            Layer::Comment => {
                base.push(pad);
                kw.push(pad);
                st.push(pad);
                cm.push(c);
            }
        }
    }

    HighlightLayers {
        base,
        keywords: kw,
        strings: st,
        comments: cm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_are_char_aligned() {
        let src = "fn main() { // 入口\n    let s = \"你好\"; }\n";
        let h = highlight(src, "rs");
        let count = |s: &str| s.chars().count();
        assert_eq!(count(&h.base), count(&h.keywords));
        assert_eq!(count(&h.base), count(&h.strings));
        assert_eq!(count(&h.base), count(&h.comments));
        // 关键字层含 fn/let，注释层含中文注释，字符串层含引号内容
        assert!(h.keywords.contains("fn"));
        assert!(h.keywords.contains("let"));
        assert!(h.comments.contains("入口"));
        assert!(h.strings.contains("你好"));
        // base 层不再含关键字文本
        assert!(!h.base.contains("fn"));
    }

    #[test]
    fn unknown_ext_goes_plain() {
        let h = highlight("hello fn world", "xyz");
        assert_eq!(h.base, "hello fn world");
        assert!(h.keywords.trim().is_empty());
    }

    #[test]
    fn python_hash_comment() {
        let h = highlight("x = 1  # note\n", "py");
        assert!(h.comments.contains("# note"));
        assert!(h.keywords.contains('1'));
    }
}

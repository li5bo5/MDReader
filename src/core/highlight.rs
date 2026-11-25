//! 零正则轻量多语言词法高亮状态机。
//!
//! 专为 DirectWrite 渲染管线设计：
//! - 严禁引入重型正则引擎或外部语法树依赖，保持极低内存占用；
//! - 单遍扫描按严格 UTF-8 字符边界步进，确保对任意多字节中文、Emoji 或截断语法永不 panic；
//! - 合并相邻同 `TokenKind` 片段以压缩 `Vec<RichSpan>` 规模，并保证
//!   `spans.iter().map(|s| s.text.as_str()).collect::<String>() == code` 恒成立。

use crate::core::markdown::{RichSpan, TokenKind};

/// 内部识别的 8 大主流语言家族。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    Cpp,
    JsTs,
    Python,
    Json,
    Shell,
    Sql,
    Markup,
}

/// 根据代码围栏语言标识映射语言家族。
fn detect_lang(language: Option<&str>) -> Option<Lang> {
    let lang = language?.trim();
    if lang.is_empty() {
        return None;
    }
    match lang.to_ascii_lowercase().as_str() {
        "rs" | "rust" => Some(Lang::Rust),
        "c" | "h" | "cpp" | "cxx" | "cc" | "hpp" | "hxx" | "c++" => Some(Lang::Cpp),
        "js" | "javascript" | "mjs" | "cjs" | "ts" | "typescript" | "mts" | "cts" | "jsx"
        | "tsx" => Some(Lang::JsTs),
        "py" | "python" | "py3" => Some(Lang::Python),
        "json" | "jsonc" | "json5" => Some(Lang::Json),
        "ps1" | "powershell" | "pwsh" | "sh" | "bash" | "zsh" | "shell" => Some(Lang::Shell),
        "sql" | "sqlite" | "mysql" | "postgres" | "postgresql" | "tsql" => Some(Lang::Sql),
        "md" | "markdown" | "html" | "htm" | "xml" | "svg" => Some(Lang::Markup),
        _ => None,
    }
}

/// 相邻同类型 Token 自动合并构建器。
struct SpanBuilder {
    spans: Vec<RichSpan>,
}

impl SpanBuilder {
    fn new() -> Self {
        Self { spans: Vec::new() }
    }

    fn push(&mut self, text: &str, kind: TokenKind) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.highlight == Some(kind)
        {
            last.text.push_str(text);
            return;
        }
        self.spans.push(RichSpan {
            text: text.to_string(),
            bold: false,
            italic: false,
            strike: false,
            code: true,
            link_url: None,
            highlight: Some(kind),
        });
    }

    fn finish(self) -> Vec<RichSpan> {
        self.spans
    }
}

/// 读取当前位置的单个 UTF-8 字符字节宽度。
#[inline]
fn utf8_char_len(s: &str) -> usize {
    s.chars().next().map_or(1, char::len_utf8)
}

/// 对输入源码执行词法高亮切片，返回合并后的 `RichSpan` 序列。
///
/// 对未知语言或空语言标识整体降级为单个 `TokenKind::Plain` Span；
/// 保证输出文本拼接后与输入 `code` 逐字符完全一致。
pub fn highlight_code(language: Option<&str>, code: &str) -> Vec<RichSpan> {
    if code.is_empty() {
        return Vec::new();
    }

    let Some(lang) = detect_lang(language) else {
        return vec![RichSpan {
            text: code.to_string(),
            bold: false,
            italic: false,
            strike: false,
            code: true,
            link_url: None,
            highlight: Some(TokenKind::Plain),
        }];
    };

    match lang {
        Lang::Markup => highlight_markup(code),
        _ => highlight_generic(lang, code),
    }
}

/// 扫描通用编程语言代码（`Rust | Cpp | JsTs | Python | Json | Shell | Sql`）。
fn highlight_generic(lang: Lang, code: &str) -> Vec<RichSpan> {
    let bytes = code.as_bytes();
    let mut builder = SpanBuilder::new();
    let mut i = 0usize;

    while i < bytes.len() {
        // Step 1: 行注释与块注释
        if has_line_comment_prefix(lang, bytes, i) {
            let mut end = i;
            while end < bytes.len() && bytes[end] != b'\n' {
                end += utf8_char_len(&code[end..]);
            }
            builder.push(&code[i..end], TokenKind::Comment);
            i = end;
            continue;
        }

        if has_block_comment_start(lang, bytes, i) {
            let end = scan_block_comment(code, i, lang == Lang::Rust);
            builder.push(&code[i..end], TokenKind::Comment);
            i = end;
            continue;
        }

        // Step 2: 字符串与字符字面量
        if lang == Lang::Python
            && (code[i..].starts_with("\"\"\"") || code[i..].starts_with("'''"))
        {
            let delim = &bytes[i..i + 3];
            let end = scan_python_triple_quote(code, i, delim);
            builder.push(&code[i..end], TokenKind::String);
            i = end;
            continue;
        }

        if lang == Lang::Rust
            && let Some(end) = try_scan_rust_raw_string(code, i)
        {
            builder.push(&code[i..end], TokenKind::String);
            i = end;
            continue;
        }

        if bytes[i] == b'"' {
            let end = scan_quoted_string(code, i, b'"', true, false);
            builder.push(&code[i..end], TokenKind::String);
            i = end;
            continue;
        }

        if bytes[i] == b'\'' {
            match lang {
                Lang::JsTs | Lang::Python | Lang::Shell => {
                    let end = scan_quoted_string(code, i, b'\'', true, false);
                    builder.push(&code[i..end], TokenKind::String);
                    i = end;
                    continue;
                }
                Lang::Sql => {
                    let end = scan_quoted_string(code, i, b'\'', false, true);
                    builder.push(&code[i..end], TokenKind::String);
                    i = end;
                    continue;
                }
                Lang::Cpp => {
                    if let Some(end) = try_scan_single_line_char(code, i) {
                        builder.push(&code[i..end], TokenKind::String);
                        i = end;
                        continue;
                    }
                }
                Lang::Rust => {
                    if let Some(end) = try_scan_rust_char_literal(code, i) {
                        builder.push(&code[i..end], TokenKind::String);
                        i = end;
                        continue;
                    }
                    // 未匹配到字符字面量，视为 `'a` 生命周期起始单引号
                    builder.push(&code[i..i + 1], TokenKind::Plain);
                    i += 1;
                    continue;
                }
                Lang::Json | Lang::Markup => {}
            }
        }

        if bytes[i] == b'`' && matches!(lang, Lang::JsTs | Lang::Shell) {
            let end = scan_quoted_string(code, i, b'`', true, false);
            builder.push(&code[i..end], TokenKind::String);
            i = end;
            continue;
        }

        // Step 3: 数字字面量
        if bytes[i].is_ascii_digit() {
            let end = scan_number_literal(bytes, i);
            builder.push(&code[i..end], TokenKind::Number);
            i = end;
            continue;
        }

        // Step 4: 标识符、关键字与类型/函数
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let mut end = i + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            let ident = &code[i..end];
            let kind = if is_keyword(lang, ident) {
                TokenKind::Keyword
            } else if is_type_or_fn(lang, ident, bytes, end) {
                TokenKind::TypeOrFn
            } else {
                TokenKind::Plain
            };
            builder.push(ident, kind);
            i = end;
            continue;
        }

        // Step 5: 普通 ASCII 符号/空白与多字节 UTF-8 安全步进
        if bytes[i] < 0x80 {
            let mut end = i + 1;
            while end < bytes.len()
                && bytes[end] < 0x80
                && !may_start_special_token(lang, bytes, end)
            {
                end += 1;
            }
            builder.push(&code[i..end], TokenKind::Plain);
            i = end;
        } else {
            let ch_len = utf8_char_len(&code[i..]);
            builder.push(&code[i..i + ch_len], TokenKind::Plain);
            i += ch_len;
        }
    }

    builder.finish()
}

/// 判断指定 ASCII 字节位置是否可能触发注释/字符串/数字/标识符，以便批量合并 Plain 片段。
#[inline]
fn may_start_special_token(lang: Lang, bytes: &[u8], idx: usize) -> bool {
    let b = bytes[idx];
    b.is_ascii_alphanumeric()
        || b == b'_'
        || b == b'"'
        || b == b'\''
        || (b == b'`' && matches!(lang, Lang::JsTs | Lang::Shell))
        || (b == b'/' && matches!(lang, Lang::Rust | Lang::Cpp | Lang::JsTs | Lang::Json | Lang::Sql))
        || (b == b'#' && matches!(lang, Lang::Python | Lang::Shell))
        || (b == b'-' && lang == Lang::Sql)
}

#[inline]
fn has_line_comment_prefix(lang: Lang, bytes: &[u8], i: usize) -> bool {
    match lang {
        Lang::Rust | Lang::Cpp | Lang::JsTs | Lang::Json => {
            bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/')
        }
        Lang::Python | Lang::Shell => bytes[i] == b'#',
        Lang::Sql => bytes[i] == b'-' && bytes.get(i + 1) == Some(&b'-'),
        Lang::Markup => false,
    }
}

#[inline]
fn has_block_comment_start(lang: Lang, bytes: &[u8], i: usize) -> bool {
    matches!(lang, Lang::Rust | Lang::Cpp | Lang::JsTs | Lang::Json | Lang::Sql)
        && bytes[i] == b'/'
        && bytes.get(i + 1) == Some(&b'*')
}

/// 扫描块注释（支持 Rust 嵌套 `/* ... */` 计数）。
fn scan_block_comment(code: &str, start: usize, nested: bool) -> usize {
    let bytes = code.as_bytes();
    let mut i = start + 2;
    let mut depth = 1usize;

    while i < bytes.len() {
        if nested && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += utf8_char_len(&code[i..]);
        }
    }
    bytes.len()
}

/// 扫描 Python 三引号字符串（支持 `\` 转义与跨行）。
fn scan_python_triple_quote(code: &str, start: usize, delim: &[u8]) -> usize {
    let bytes = code.as_bytes();
    let mut i = start + 3;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 1;
            if i < bytes.len() {
                i += utf8_char_len(&code[i..]);
            }
        } else if bytes.get(i..i + 3) == Some(delim) {
            return i + 3;
        } else {
            i += utf8_char_len(&code[i..]);
        }
    }
    bytes.len()
}

/// 扫描 Rust 原始字符串 `r"..."`、`r#"..."#`、`r##"..."##`。
fn try_scan_rust_raw_string(code: &str, start: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    if bytes[start] != b'r' {
        return None;
    }

    let mut idx = start + 1;
    let mut hashes = 0usize;
    while bytes.get(idx) == Some(&b'#') {
        hashes += 1;
        idx += 1;
    }
    if bytes.get(idx) != Some(&b'"') {
        return None;
    }

    idx += 1;
    while idx < bytes.len() {
        if bytes[idx] == b'"' {
            let mut matched = 0usize;
            while matched < hashes && bytes.get(idx + 1 + matched) == Some(&b'#') {
                matched += 1;
            }
            if matched == hashes {
                return Some(idx + 1 + hashes);
            }
        }
        idx += utf8_char_len(&code[idx..]);
    }
    Some(bytes.len())
}

/// 扫描带边界引号的字符串（支持 `\` 转义及可选 SQL `''` 双引号转义）。
fn scan_quoted_string(
    code: &str,
    start: usize,
    quote: u8,
    backslash_escape: bool,
    doubled_quote_escape: bool,
) -> usize {
    let bytes = code.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        if backslash_escape && bytes[i] == b'\\' {
            i += 1;
            if i < bytes.len() {
                i += utf8_char_len(&code[i..]);
            }
        } else if bytes[i] == quote {
            if doubled_quote_escape && bytes.get(i + 1) == Some(&quote) {
                i += 2;
            } else {
                return i + 1;
            }
        } else {
            i += utf8_char_len(&code[i..]);
        }
    }
    bytes.len()
}

/// C/C++ 同行单引号字符字面量探测（未闭合则不吞跨行代码）。
fn try_scan_single_line_char(code: &str, start: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' || b == b'\r' {
            return None;
        }
        if b == b'\\' {
            i += 1;
            if i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += utf8_char_len(&code[i..]);
            } else {
                return None;
            }
        } else if b == b'\'' {
            return Some(i + 1);
        } else {
            i += utf8_char_len(&code[i..]);
        }
    }
    None
}

/// Rust 字符字面量精确识别，防止把 `'a` / `'static` 生命周期误判为字符串。
fn try_scan_rust_char_literal(code: &str, start: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    let next = *bytes.get(start + 1)?;
    if next == b'\n' || next == b'\r' {
        return None;
    }

    if next == b'\\' {
        let mut i = start + 2;
        if i >= bytes.len() || bytes[i] == b'\n' || bytes[i] == b'\r' {
            return None;
        }
        i += utf8_char_len(&code[i..]);
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'\'' {
                return Some(i + 1);
            }
            if b == b'\n' || b == b'\r' || b == b' ' || b == b'\t' {
                return None;
            }
            i += utf8_char_len(&code[i..]);
        }
        return None;
    }

    if next == b'\'' {
        return None;
    }

    let ch_len = utf8_char_len(&code[start + 1..]);
    let close_idx = start + 1 + ch_len;
    if bytes.get(close_idx) == Some(&b'\'') {
        Some(close_idx + 1)
    } else {
        None
    }
}

/// 扫描数字字面量（支持进制前缀、`_` 分隔、安全小数点判断、科学计数法与类型后缀）。
fn scan_number_literal(bytes: &[u8], start: usize) -> usize {
    let mut i = start;

    if bytes[i] == b'0'
        && let Some(&prefix) = bytes.get(i + 1)
    {
        match prefix {
            b'x' | b'X' => {
                i += 2;
                while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') {
                    i += 1;
                }
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                return i;
            }
            b'b' | b'B' => {
                i += 2;
                while i < bytes.len() && (matches!(bytes[i], b'0' | b'1' | b'_')) {
                    i += 1;
                }
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                return i;
            }
            b'o' | b'O' => {
                i += 2;
                while i < bytes.len() && (matches!(bytes[i], b'0'..=b'7' | b'_')) {
                    i += 1;
                }
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                return i;
            }
            _ => {}
        }
    }

    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }

    // 仅当 `.` 后紧跟数字时才吞入小数点，避免把 Rust `1..10` 破坏为数字
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
    }

    // 科学计数法指数部分
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut exp_idx = i + 1;
        if matches!(bytes.get(exp_idx), Some(b'+' | b'-')) {
            exp_idx += 1;
        }
        if bytes.get(exp_idx).is_some_and(u8::is_ascii_digit) {
            i = exp_idx;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }

    // 整数/浮点后缀（如 `u64`, `f32`, `ULL`）
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }

    i
}

/// 判断标识符是否属于对应语言的关键字。
fn is_keyword(lang: Lang, ident: &str) -> bool {
    match lang {
        Lang::Rust => matches!(
            ident,
            "as" | "async"
                | "await"
                | "break"
                | "const"
                | "continue"
                | "crate"
                | "dyn"
                | "else"
                | "enum"
                | "extern"
                | "false"
                | "fn"
                | "for"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "loop"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "pub"
                | "ref"
                | "return"
                | "self"
                | "Self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "true"
                | "type"
                | "unsafe"
                | "use"
                | "where"
                | "while"
                | "try"
                | "macro_rules"
        ),
        Lang::Cpp => matches!(
            ident,
            "auto"
                | "bool"
                | "break"
                | "case"
                | "catch"
                | "char"
                | "class"
                | "const"
                | "constexpr"
                | "continue"
                | "default"
                | "delete"
                | "do"
                | "double"
                | "else"
                | "enum"
                | "explicit"
                | "export"
                | "extern"
                | "false"
                | "float"
                | "for"
                | "friend"
                | "goto"
                | "if"
                | "inline"
                | "int"
                | "long"
                | "mutable"
                | "namespace"
                | "new"
                | "noexcept"
                | "nullptr"
                | "operator"
                | "override"
                | "private"
                | "protected"
                | "public"
                | "register"
                | "return"
                | "short"
                | "signed"
                | "sizeof"
                | "static"
                | "struct"
                | "switch"
                | "template"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "typedef"
                | "typename"
                | "union"
                | "unsigned"
                | "using"
                | "virtual"
                | "void"
                | "volatile"
                | "while"
                | "include"
                | "define"
        ),
        Lang::JsTs => matches!(
            ident,
            "as" | "async"
                | "await"
                | "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "enum"
                | "export"
                | "extends"
                | "false"
                | "finally"
                | "for"
                | "from"
                | "function"
                | "if"
                | "implements"
                | "import"
                | "in"
                | "instanceof"
                | "interface"
                | "let"
                | "new"
                | "null"
                | "of"
                | "package"
                | "private"
                | "protected"
                | "public"
                | "readonly"
                | "return"
                | "static"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "type"
                | "typeof"
                | "undefined"
                | "var"
                | "void"
                | "while"
                | "with"
                | "yield"
        ),
        Lang::Python => matches!(
            ident,
            "and"
                | "as"
                | "assert"
                | "async"
                | "await"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "False"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "None"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "True"
                | "try"
                | "while"
                | "with"
                | "yield"
                | "match"
                | "case"
        ),
        Lang::Json => matches!(ident, "true" | "false" | "null"),
        Lang::Shell => matches!(
            ident,
            "if" | "then"
                | "else"
                | "elif"
                | "fi"
                | "for"
                | "in"
                | "do"
                | "done"
                | "while"
                | "until"
                | "case"
                | "esac"
                | "function"
                | "return"
                | "local"
                | "export"
                | "source"
                | "exit"
                | "true"
                | "false"
                | "param"
                | "begin"
                | "process"
                | "end"
                | "try"
                | "catch"
                | "finally"
                | "throw"
                | "foreach"
                | "switch"
                | "break"
                | "continue"
        ),
        Lang::Sql => {
            const SQL_KEYWORDS: &[&str] = &[
                "select",
                "from",
                "where",
                "insert",
                "into",
                "values",
                "update",
                "set",
                "delete",
                "create",
                "table",
                "alter",
                "drop",
                "index",
                "join",
                "inner",
                "left",
                "right",
                "full",
                "outer",
                "on",
                "as",
                "and",
                "or",
                "not",
                "null",
                "is",
                "in",
                "like",
                "between",
                "group",
                "by",
                "having",
                "order",
                "asc",
                "desc",
                "limit",
                "offset",
                "union",
                "all",
                "distinct",
                "case",
                "when",
                "then",
                "else",
                "end",
                "primary",
                "key",
                "foreign",
                "references",
                "default",
                "true",
                "false",
                "begin",
                "commit",
                "rollback",
            ];
            SQL_KEYWORDS
                .iter()
                .any(|kw| ident.eq_ignore_ascii_case(kw))
        }
        Lang::Markup => false,
    }
}

/// 判断非关键字标识符是否应高亮为类型或函数/宏调用。
fn is_type_or_fn(lang: Lang, ident: &str, bytes: &[u8], end: usize) -> bool {
    if ident.as_bytes()[0].is_ascii_uppercase() {
        return true;
    }

    let mut lookahead = end;
    if lang == Lang::Rust && bytes.get(lookahead) == Some(&b'!') {
        lookahead += 1;
        while matches!(bytes.get(lookahead), Some(b' ' | b'\t')) {
            lookahead += 1;
        }
        return matches!(bytes.get(lookahead), Some(b'(' | b'[' | b'{'));
    }

    while matches!(bytes.get(lookahead), Some(b' ' | b'\t')) {
        lookahead += 1;
    }
    bytes.get(lookahead) == Some(&b'(')
}

/// 轻量扫描标记语言（HTML / XML / SVG / Markdown）。
fn highlight_markup(code: &str) -> Vec<RichSpan> {
    let bytes = code.as_bytes();
    let mut builder = SpanBuilder::new();
    let mut i = 0usize;
    let mut at_line_indent = true;

    while i < bytes.len() {
        if code[i..].starts_with("<!--") {
            let mut end = i + 4;
            while end < bytes.len() && !code[end..].starts_with("-->") {
                end += utf8_char_len(&code[end..]);
            }
            if end < bytes.len() {
                end += 3;
            }
            builder.push(&code[i..end], TokenKind::Comment);
            i = end;
            at_line_indent = false;
            continue;
        }

        if bytes[i] == b'<'
            && bytes.get(i + 1).is_some_and(|&b| {
                b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?')
            })
        {
            at_line_indent = false;
            let mut tag_end = i + 1;
            if matches!(bytes.get(tag_end), Some(b'/' | b'!' | b'?')) {
                tag_end += 1;
            }
            while tag_end < bytes.len()
                && (bytes[tag_end].is_ascii_alphanumeric()
                    || matches!(bytes[tag_end], b'_' | b'-' | b':'))
            {
                tag_end += 1;
            }
            builder.push(&code[i..tag_end], TokenKind::Keyword);
            i = tag_end;

            // 扫描标签内部属性值直到 `>`
            while i < bytes.len() {
                if bytes[i] == b'>' {
                    builder.push(&code[i..i + 1], TokenKind::Plain);
                    i += 1;
                    break;
                }
                if bytes[i] == b'"' || bytes[i] == b'\'' {
                    let q = bytes[i];
                    let end = scan_quoted_string(code, i, q, true, false);
                    builder.push(&code[i..end], TokenKind::String);
                    i = end;
                } else {
                    let ch_len = utf8_char_len(&code[i..]);
                    builder.push(&code[i..i + ch_len], TokenKind::Plain);
                    i += ch_len;
                }
            }
            continue;
        }

        if at_line_indent && bytes[i] == b'#' {
            let mut end = i;
            while end < bytes.len() && bytes[end] != b'\n' {
                end += utf8_char_len(&code[end..]);
            }
            builder.push(&code[i..end], TokenKind::Keyword);
            i = end;
            at_line_indent = false;
            continue;
        }

        if bytes[i] == b'`' {
            let mut end = i + 1;
            let mut closed = false;
            while end < bytes.len() && bytes[end] != b'\n' {
                if bytes[end] == b'`' {
                    end += 1;
                    closed = true;
                    break;
                }
                end += utf8_char_len(&code[end..]);
            }
            if closed {
                builder.push(&code[i..end], TokenKind::String);
                i = end;
                at_line_indent = false;
                continue;
            }
        }

        if bytes[i] == b'\n' {
            builder.push(&code[i..i + 1], TokenKind::Plain);
            i += 1;
            at_line_indent = true;
            continue;
        }

        if !matches!(bytes[i], b' ' | b'\t') {
            at_line_indent = false;
        }
        let ch_len = utf8_char_len(&code[i..]);
        builder.push(&code[i..i + ch_len], TokenKind::Plain);
        i += ch_len;
    }

    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn concat_spans(spans: &[RichSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn test_unknown_and_empty_language_fallback_to_single_plain_span() {
        assert!(highlight_code(Some("rust"), "").is_empty());

        let code = "fn main() { println!(\"hi\"); }";
        let unknown = highlight_code(Some("brainfuck"), code);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].text, code);
        assert_eq!(unknown[0].highlight, Some(TokenKind::Plain));
        assert!(unknown[0].code);

        let none_lang = highlight_code(None, code);
        assert_eq!(none_lang.len(), 1);
        assert_eq!(none_lang[0].text, code);
    }

    #[test]
    fn test_all_eight_language_families_mapped_and_highlighted() {
        let cases = [
            ("rs", "pub fn f() -> u32 { 42 }", TokenKind::Keyword),
            ("cpp", "class Foo { int x = 1; };", TokenKind::Keyword),
            ("tsx", "const s = `hi`;", TokenKind::Keyword),
            ("python", "def greet(): return \"你好\"", TokenKind::Keyword),
            ("json", "{\"ok\": true, \"n\": 12}", TokenKind::Keyword),
            ("powershell", "if ($true) { 'ok' }", TokenKind::Keyword),
            ("sql", "SELECT id FROM users WHERE name = 'a''b';", TokenKind::Keyword),
            ("html", "<!-- c --><div class=\"x\">hi</div>", TokenKind::Comment),
        ];

        for (lang, src, expected_kind) in cases {
            let spans = highlight_code(Some(lang), src);
            assert_eq!(concat_spans(&spans), src, "语言 {lang} 必须 100% 还原源码");
            assert!(
                spans.iter().any(|s| s.highlight == Some(expected_kind)),
                "语言 {lang} 应识别预期 token"
            );
        }
    }

    #[test]
    fn test_adjacent_same_token_kinds_are_merged() {
        let src = "a + b * c";
        let spans = highlight_code(Some("rust"), src);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].highlight, Some(TokenKind::Plain));
        assert_eq!(spans[0].text, src);
    }

    #[test]
    fn test_rust_lifetime_range_raw_string_and_nested_comments() {
        let src = r###"fn parse<'a>(s: &'a str) -> Vec<usize> {
    /* outer /* inner */ still comment */
    let ch = '\n';
    let emoji = '🦀';
    let raw1 = r#"hello "world""#;
    let raw2 = r##"nested "# ok"##;
    let rng = 1..10;
    let float = 3.14f64;
    println!("中文\\🦀");
    vec![1]
}"###;

        let spans = highlight_code(Some("rust"), src);
        assert_eq!(concat_spans(&spans), src);

        // 验证 `'a` 生命周期没有把后续整段吞成字符串
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::Keyword) && s.text == "let"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::Comment) && s.text.contains("still comment")));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::String) && s.text == "'\\n'"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::String) && s.text == "'🦀'"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::String) && s.text == "r#\"hello \"world\"\"#"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::String) && s.text == "r##\"nested \"# ok\"##"));
        // 验证 `1..10` 被拆为 `1`, `..`, `10` 而非错误浮点
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::Number) && s.text == "1"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::Number) && s.text == "10"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::Number) && s.text == "3.14f64"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::TypeOrFn) && s.text == "println"));
    }

    #[test]
    fn test_python_triple_quote_and_unterminated_tokens_safe() {
        let src = "# 中文注释 🚀\ndoc = \"\"\"line1\n\\\"quoted\\\"\nline2\"\"\"\ns = \"unterminated \\中";
        let spans = highlight_code(Some("py"), src);
        assert_eq!(concat_spans(&spans), src);
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::Comment) && s.text.contains("中文注释")));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::String) && s.text.contains("line2")));
    }

    #[test]
    fn test_markup_markdown_and_xml_lossless() {
        let src = "# 标题\n使用 `code` 与 <span id=\"v\">内容</span> <!-- 注释 -->";
        let spans = highlight_code(Some("markdown"), src);
        assert_eq!(concat_spans(&spans), src);
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::Keyword) && s.text == "# 标题"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::String) && s.text == "`code`"));
        assert!(spans.iter().any(|s| s.highlight == Some(TokenKind::Comment) && s.text == "<!-- 注释 -->"));
    }
}

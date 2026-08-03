//! Inline LaTeX-to-Unicode conversion for terminal display.
//!
//! Terminals cannot lay out real math, but a useful subset of LaTeX maps
//! cleanly onto Unicode: Greek letters (`\alpha`→`α`), big operators
//! (`\sum`→`∑`), relations (`\leq`→`≤`), arrows (`\rightarrow`→`→`), and
//! subscripts/superscripts (`x_1`→`x₁`, `x^2`→`x²`). This module turns
//! a LaTeX math fragment into a Unicode string suitable for rendering.
//!
//! ## What is supported
//!
//! - Greek letters (lowercase `\alpha`–`\omega`, uppercase `\Alpha`–`\Omega`)
//! - Math operators (`\times`, `\div`, `\pm`, `\cdot`, …)
//! - Relations (`\leq`, `\geq`, `\neq`, `\approx`, `\equiv`, `\infty`, …)
//! - Arrows (`\rightarrow`, `\Leftarrow`, `\Leftrightarrow`, …)
//! - Set notation (`\in`, `\cup`, `\cap`, `\emptyset`, …)
//! - Subscripts/superscripts via `_` and `^` with single chars or braced groups
//! - Accents (`\'e`→`é`, `` \`e ``→`è`, `\^e`→`ê`, `\"e`→`ë`, `\~n`→`ñ`, …)
//! - Common symbols (`\degree`→`°`, `\checkmark`→`✓`, `\dots`→`…`, …)
//!
//! ## What is NOT supported
//!
//! This is a single-pass inline renderer. Constructs requiring layout
//! (matrices, environments, multi-argument commands) are passed through
//! literally — terminals can't lay them out anyway.
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Single-character lookup for `\name` commands to their Unicode glyph.
///
/// Curated subset of the most common math symbols. Anything not here falls
/// through to the raw command name.
const SYMBOLS: &[(&str, &str)] = &[
    // -----------------------------------------------------------------------
    // Greek lowercase
    // -----------------------------------------------------------------------
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ϵ"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("vartheta", "ϑ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("omicron", "ο"),
    ("pi", "π"),
    ("rho", "ρ"),
    ("sigma", "σ"),
    ("varsigma", "ς"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "ϕ"),
    ("varphi", "φ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    // -----------------------------------------------------------------------
    // Greek uppercase
    // -----------------------------------------------------------------------
    ("Alpha", "Α"),
    ("Beta", "Β"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Epsilon", "Ε"),
    ("Zeta", "Ζ"),
    ("Eta", "Η"),
    ("Theta", "Θ"),
    ("Iota", "Ι"),
    ("Kappa", "Κ"),
    ("Lambda", "Λ"),
    ("Mu", "Μ"),
    ("Nu", "Ν"),
    ("Xi", "Ξ"),
    ("Omicron", "Ο"),
    ("Pi", "Π"),
    ("Rho", "Ρ"),
    ("Sigma", "Σ"),
    ("Tau", "Τ"),
    ("Upsilon", "Υ"),
    ("Phi", "Φ"),
    ("Chi", "Χ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    // -----------------------------------------------------------------------
    // Big operators
    // -----------------------------------------------------------------------
    ("sum", "∑"),
    ("prod", "∏"),
    ("coprod", "∐"),
    ("int", "∫"),
    ("iint", "∬"),
    ("iiint", "∭"),
    ("oint", "∮"),
    ("bigcap", "⋂"),
    ("bigcup", "⋃"),
    // -----------------------------------------------------------------------
    // Binary operators
    // -----------------------------------------------------------------------
    ("pm", "±"),
    ("mp", "∓"),
    ("times", "×"),
    ("div", "÷"),
    ("ast", "∗"),
    ("star", "⋆"),
    ("circ", "∘"),
    ("bullet", "•"),
    ("cdot", "⋅"),
    ("centerdot", "·"),
    ("cap", "∩"),
    ("cup", "∪"),
    ("oplus", "⊕"),
    ("ominus", "⊖"),
    ("otimes", "⊗"),
    ("oslash", "⊘"),
    ("odot", "⊙"),
    ("dagger", "†"),
    ("ddagger", "‡"),
    ("wr", "≀"),
    ("diamond", "⋄"),
    // -----------------------------------------------------------------------
    // Relations
    // -----------------------------------------------------------------------
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("ll", "≪"),
    ("gg", "≫"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("equiv", "≡"),
    ("sim", "∼"),
    ("simeq", "≃"),
    ("approx", "≈"),
    ("cong", "≅"),
    ("propto", "∝"),
    ("asymp", "≍"),
    ("prec", "≺"),
    ("succ", "≻"),
    ("subset", "⊂"),
    ("supset", "⊃"),
    ("subseteq", "⊆"),
    ("supseteq", "⊇"),
    ("in", "∈"),
    ("notin", "∉"),
    ("ni", "∋"),
    ("mid", "∣"),
    ("parallel", "∥"),
    ("perp", "⊥"),
    // -----------------------------------------------------------------------
    // Arrows
    // -----------------------------------------------------------------------
    ("leftarrow", "←"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftrightarrow", "↔"),
    ("Leftarrow", "⇐"),
    ("Rightarrow", "⇒"),
    ("Leftrightarrow", "⇔"),
    ("uparrow", "↑"),
    ("downarrow", "↓"),
    ("updownarrow", "↕"),
    ("Uparrow", "⇑"),
    ("Downarrow", "⇓"),
    ("Updownarrow", "⇕"),
    ("mapsto", "↦"),
    ("hookleftarrow", "↩"),
    ("hookrightarrow", "↪"),
    ("implies", "⟹"),
    ("iff", "⟺"),
    // -----------------------------------------------------------------------
    // Misc symbols
    // -----------------------------------------------------------------------
    ("infty", "∞"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("nexists", "∄"),
    ("emptyset", "∅"),
    ("varnothing", "∅"),
    ("angle", "∠"),
    ("triangle", "△"),
    ("square", "□"),
    ("ldots", "…"),
    ("dots", "…"),
    ("cdots", "⋯"),
    ("degree", "°"),
    ("checkmark", "✓"),
    ("hbar", "ℏ"),
    ("ell", "ℓ"),
    ("Re", "ℜ"),
    ("Im", "ℑ"),
    ("wp", "℘"),
    ("aleph", "ℵ"),
    ("mho", "℧"),
];

/// Unicode subscript forms keyed by single char.
const SUBSCRIPT: &[(char, &str)] = &[
    ('0', "₀"),
    ('1', "₁"),
    ('2', "₂"),
    ('3', "₃"),
    ('4', "₄"),
    ('5', "₅"),
    ('6', "₆"),
    ('7', "₇"),
    ('8', "₈"),
    ('9', "₉"),
    ('+', "₊"),
    ('-', "₋"),
    ('=', "₌"),
    ('(', "₍"),
    (')', "₎"),
    ('a', "ₐ"),
    ('e', "ₑ"),
    ('h', "ₕ"),
    ('i', "ᵢ"),
    ('j', "ⱼ"),
    ('k', "ₖ"),
    ('l', "ₗ"),
    ('m', "ₘ"),
    ('n', "ₙ"),
    ('o', "ₒ"),
    ('p', "ₚ"),
    ('r', "ᵣ"),
    ('s', "ₛ"),
    ('t', "ₜ"),
    ('u', "ᵤ"),
    ('v', "ᵥ"),
    ('x', "ₓ"),
];

/// Unicode superscript forms keyed by single char.
const SUPERSCRIPT: &[(char, &str)] = &[
    ('0', "⁰"),
    ('1', "¹"),
    ('2', "²"),
    ('3', "³"),
    ('4', "⁴"),
    ('5', "⁵"),
    ('6', "⁶"),
    ('7', "⁷"),
    ('8', "⁸"),
    ('9', "⁹"),
    ('+', "⁺"),
    ('-', "⁻"),
    ('=', "⁼"),
    ('(', "⁽"),
    (')', "⁾"),
    ('a', "ᵃ"),
    ('b', "ᵇ"),
    ('c', "ᶜ"),
    ('d', "ᵈ"),
    ('e', "ᵉ"),
    ('f', "ᶠ"),
    ('g', "ᵍ"),
    ('h', "ʰ"),
    ('i', "ⁱ"),
    ('j', "ʲ"),
    ('k', "ᵏ"),
    ('l', "ˡ"),
    ('m', "ᵐ"),
    ('n', "ⁿ"),
    ('o', "ᵒ"),
    ('p', "ᵖ"),
    ('r', "ʳ"),
    ('s', "ˢ"),
    ('t', "ᵗ"),
    ('u', "ᵘ"),
    ('v', "ᵛ"),
    ('w', "ʷ"),
    ('x', "ˣ"),
    ('y', "ʸ"),
    ('z', "ᶻ"),
];

/// Math functions whose name is rendered verbatim.
const FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "sinh", "cosh", "tanh", "coth", "arcsin", "arccos",
    "arctan", "ln", "log", "exp", "lim", "limsup", "liminf", "max", "min", "sup", "inf", "det",
    "dim", "ker", "hom", "arg", "deg", "gcd", "lcm", "Pr", "mod",
];

/// Precomposed Latin accented glyphs keyed by `(accent_char, base_char)`.
///
/// Used for the single-char escape accents (`\'e` → `é`, `` \`a `` → `à`,
/// etc.). Covers the most common Latin vowel + consonant combinations; any
/// `(accent, base)` pair not here falls through to the decomposed form
/// (base + combining mark), which most terminals render correctly.
const PRECOMPOSED_ACCENTS: &[(u8, char, &str)] = &[
    // acute ('): á é í ó ú ý Á É Í Ó Ú Ý
    (b'\'', 'a', "á"),
    (b'\'', 'e', "é"),
    (b'\'', 'i', "í"),
    (b'\'', 'o', "ó"),
    (b'\'', 'u', "ú"),
    (b'\'', 'y', "ý"),
    (b'\'', 'A', "Á"),
    (b'\'', 'E', "É"),
    (b'\'', 'I', "Í"),
    (b'\'', 'O', "Ó"),
    (b'\'', 'U', "Ú"),
    (b'\'', 'Y', "Ý"),
    (b'\'', 'c', "ć"),
    (b'\'', 's', "ś"),
    (b'\'', 'z', "ź"),
    (b'\'', 'n', "ń"),
    // grave (`): à è ì ò ù À È Ì Ò Ù
    (b'`', 'a', "à"),
    (b'`', 'e', "è"),
    (b'`', 'i', "ì"),
    (b'`', 'o', "ò"),
    (b'`', 'u', "ù"),
    (b'`', 'A', "À"),
    (b'`', 'E', "È"),
    (b'`', 'I', "Ì"),
    (b'`', 'O', "Ò"),
    (b'`', 'U', "Ù"),
    // circumflex (^): â ê î ô û Â Ê Î Ô Û
    (b'^', 'a', "â"),
    (b'^', 'e', "ê"),
    (b'^', 'i', "î"),
    (b'^', 'o', "ô"),
    (b'^', 'u', "û"),
    (b'^', 'A', "Â"),
    (b'^', 'E', "Ê"),
    (b'^', 'I', "Î"),
    (b'^', 'O', "Ô"),
    (b'^', 'U', "Û"),
    // diaeresis ("): ä ë ï ö ü ÿ Ä Ë Ï Ö Ü
    (b'"', 'a', "ä"),
    (b'"', 'e', "ë"),
    (b'"', 'i', "ï"),
    (b'"', 'o', "ö"),
    (b'"', 'u', "ü"),
    (b'"', 'y', "ÿ"),
    (b'"', 'A', "Ä"),
    (b'"', 'E', "Ë"),
    (b'"', 'I', "Ï"),
    (b'"', 'O', "Ö"),
    (b'"', 'U', "Ü"),
    // tilde (~): ñ Ñ ã õ Ã Õ
    (b'~', 'n', "ñ"),
    (b'~', 'N', "Ñ"),
    (b'~', 'a', "ã"),
    (b'~', 'o', "õ"),
    (b'~', 'A', "Ã"),
    (b'~', 'O', "Õ"),
    // cedilla (c): ç Ç
    (b'c', 'c', "ç"),
    (b'c', 'C', "Ç"),
];

/// Combining mark for each single-char accent escape. The mark itself is
/// appended after the base char when no precomposed glyph is available.
const ACCENT_COMBINING: &[(u8, &str)] = &[
    (b'\'', "\u{0301}"), // acute
    (b'`', "\u{0300}"),  // grave
    (b'^', "\u{0302}"),  // circumflex
    (b'"', "\u{0308}"),  // diaeresis
    (b'~', "\u{0303}"),  // tilde
    (b'=', "\u{0304}"),  // macron
    (b'u', "\u{0306}"),  // breve
    (b'.', "\u{0307}"),  // dot above
    (b'v', "\u{030C}"),  // caron
    (b'H', "\u{030B}"),  // double acute
    (b'k', "\u{0328}"),  // ogonek
    (b'r', "\u{030A}"),  // ring above
];

/// Named accent commands (`\hat`, `\bar`, `\tilde`, …) → combining mark.
fn accent_combining(name: &str) -> Option<&'static str> {
    let mark: Option<&str> = match name {
        "hat" | "widehat" => Some("\u{0302}"),
        "check" | "widecheck" => Some("\u{030C}"),
        "tilde" | "widetilde" => Some("\u{0303}"),
        "acute" => Some("\u{0301}"),
        "grave" => Some("\u{0300}"),
        "dot" => Some("\u{0307}"),
        "ddot" => Some("\u{0308}"),
        "dddot" => Some("\u{20DB}"),
        "ddddot" => Some("\u{20DC}"),
        "breve" => Some("\u{0306}"),
        "bar" | "overline" => Some("\u{0304}"),
        "vec" | "overrightarrow" => Some("\u{20D7}"),
        "overleftarrow" => Some("\u{20D6}"),
        "mathring" => Some("\u{030A}"),
        "underline" | "underbar" => Some("\u{0332}"),
        _ => None,
    };
    mark
}

/// Pre-built symbol table for O(1) lookup.
static SYMBOL_TABLE: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| SYMBOLS.iter().copied().collect());

/// Pre-built subscript table for O(1) lookup.
static SUBSCRIPT_TABLE: LazyLock<HashMap<char, &'static str>> =
    LazyLock::new(|| SUBSCRIPT.iter().copied().collect());

/// Pre-built superscript table for O(1) lookup.
static SUPERSCRIPT_TABLE: LazyLock<HashMap<char, &'static str>> =
    LazyLock::new(|| SUPERSCRIPT.iter().copied().collect());

/// Pre-built function-name set for O(1) lookup.
static FUNCTION_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| FUNCTIONS.iter().copied().collect());

/// Pre-built precomposed-accent table for O(1) lookup by (accent, base).
static PRECOMPOSED_ACCENT_TABLE: LazyLock<HashMap<(u8, char), &'static str>> =
    LazyLock::new(|| {
        PRECOMPOSED_ACCENTS
            .iter()
            .map(|&(a, b, g)| ((a, b), g))
            .collect()
    });

/// Pre-built combining-mark table for O(1) lookup by accent byte.
static ACCENT_COMBINING_TABLE: LazyLock<HashMap<u8, &'static str>> =
    LazyLock::new(|| ACCENT_COMBINING.iter().copied().collect());

/// Quick check whether a string likely contains LaTeX commands.
///
/// Scans for `\` followed by an ASCII letter, or `^`/`_` followed by a digit
/// or brace (the latter is heuristic — `_1` in prose isn't LaTeX, but
/// `x_1` is; we keep this simple). Returns `false` on empty input.
#[must_use]
pub fn has_latex(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                // Backslash followed by an ASCII letter = named command.
                if i + 1 < bytes.len() && bytes[i + 1].is_ascii_alphabetic() {
                    return true;
                }
                // Single-char escapes (`\,`, `\;`, `\.`, etc.) — also a sign.
                if i + 1 < bytes.len() && !bytes[i + 1].is_ascii_alphabetic() {
                    return true;
                }
                i += 1;
            }
            b'^' | b'_' => {
                // Script char immediately followed by a digit, letter, brace,
                // or backslash (i.e. an actual subscript/superscript payload).
                if i + 1 < bytes.len() {
                    let next = bytes[i + 1];
                    if next.is_ascii_alphanumeric() || next == b'{' || next == b'\\' {
                        return true;
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    false
}

/// Apply a single-char accent to one base char: precomposed glyph when
/// available, decomposed (base + combining mark) otherwise. Returns
/// `None` when `accent` isn't a recognised accent byte.
fn apply_accent_char(accent: u8, base: char) -> Option<String> {
    if let Some(&g) = PRECOMPOSED_ACCENT_TABLE.get(&(accent, base)) {
        return Some(g.to_string());
    }
    let &combining = ACCENT_COMBINING_TABLE.get(&accent)?;
    let mut s = String::with_capacity(base.len_utf8() + combining.len());
    s.push(base);
    s.push_str(combining);
    Some(s)
}

/// Convert a LaTeX math fragment to its Unicode rendering.
///
/// Performs a single left-to-right pass over `input`. Recognized constructs:
///
/// - `\name` named commands (Greek letters, math operators, …)
/// - `\_` and `\^` script syntax (single char or `{group}`)
/// - Single-char escape accents (`\'e` → `é`, `` \`a `` → `à`, `\^o` → `ô`,
///   `\"u` → `ü`, `\~n` → `ñ`, `\c{c}` → `ç`, …) consume the next base char
/// - `\accent{base}` / `\accent base` named forms (`\hat x` → `x̂`, `\bar y` → `ȳ`)
/// - `{...}` groups (parsed recursively)
/// - `\frac{a}{b}` and `\sqrt{x}` render as inline parenthesised forms
///
/// Stray braces, math delimiters (`$`, `\(`, `\[`), and stray backslashes
/// are dropped or passed through as appropriate.
///
/// Unknown commands are left as their bare name (e.g. `\foo` → `foo`) so the
/// output is still readable.
#[must_use]
pub fn latex_to_unicode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    out.push('\\');
                    break;
                }
                let c = bytes[i];
                if c.is_ascii_alphabetic() {
                    // Read command name (letters only).
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let name = &input[start..i];
                    // Look up symbol.
                    if let Some(&glyph) = SYMBOL_TABLE.get(name) {
                        out.push_str(glyph);
                    } else if FUNCTION_SET.contains(name) {
                        // Function names render verbatim (sin, cos, lim, …).
                        out.push_str(name);
                    } else if let Some(combining) = accent_combining(name) {
                        // Named accent: \hat, \bar, \tilde, …
                        i = parse_named_accent(input, i, &mut out, combining);
                    } else if name == "frac" {
                        // Inline fractions: render as (num)/(den). Braces
                        // stripped; nested groups are recursed.
                        i = parse_frac(input, i, &mut out);
                    } else if name == "sqrt" {
                        i = parse_sqrt(input, i, &mut out);
                    } else {
                        // Unknown command: emit bare name so it remains readable.
                        out.push_str(name);
                    }
                } else if let Some(&combining) = ACCENT_COMBINING_TABLE.get(&c) {
                    // Single-char escape accent: consume the next base char
                    // (or braced group) and apply the mark.
                    i += 1;
                    if i >= bytes.len() {
                        // Trailing accent with no payload — emit nothing.
                        break;
                    }
                    if bytes[i] == b'{' {
                        let end = find_matching_brace(input, i + 1);
                        let inner = &input[i + 1..end.min(input.len())];
                        for ch in inner.chars() {
                            if let Some(acc) = apply_accent_char(c, ch) {
                                out.push_str(&acc);
                            } else {
                                out.push(ch);
                                out.push_str(combining);
                            }
                        }
                        i = if end < input.len() {
                            end + 1
                        } else {
                            input.len()
                        };
                    } else {
                        let base = bytes[i] as char;
                        if let Some(acc) = apply_accent_char(c, base) {
                            out.push_str(&acc);
                        } else {
                            out.push(base);
                            out.push_str(combining);
                        }
                        i += 1;
                    }
                } else {
                    // Layout / structural single-char escape.
                    match c {
                        b'\\' => out.push('\n'),                    // row break
                        b'n' => out.push('\n'),                     // explicit newline
                        b',' | b':' | b';' | b'>' => out.push(' '), // spacing
                        b'!' => {}                                  // negative thin space
                        b'|' => out.push('‖'),
                        b'%' => out.push('%'),
                        b'#' => out.push('#'),
                        b'$' => out.push('$'),
                        b'&' => out.push('&'),
                        b'_' => out.push('_'),
                        b'{' => out.push('{'),
                        b'}' => out.push('}'),
                        b' ' => out.push(' '),
                        b'.' => out.push('.'),
                        _ => out.push(c as char),
                    }
                    i += 1;
                }
            }
            b'^' => {
                i += 1;
                parse_script(input, i, &mut out, true);
                i = skip_script_payload(input, i);
            }
            b'_' => {
                i += 1;
                parse_script(input, i, &mut out, false);
                i = skip_script_payload(input, i);
            }
            b'{' => {
                // Consume a balanced group, recursing into latex_to_unicode.
                i += 1;
                let group_end = find_matching_brace(input, i);
                let inner = &input[i..group_end];
                out.push_str(&latex_to_unicode(inner));
                i = if group_end < input.len() {
                    group_end + 1
                } else {
                    group_end
                };
            }
            b'}' => {
                // Stray close brace (no matching opener in this scope).
                i += 1;
            }
            b'$' | b'&' | b'#' | b'%' => {
                // Math/layout metacharacters: drop in inline mode.
                i += 1;
            }
            _ => {
                // Plain character: run of non-special bytes.
                let start = i;
                while i < bytes.len()
                    && !matches!(
                        bytes[i],
                        b'\\' | b'^' | b'_' | b'{' | b'}' | b'$' | b'&' | b'#' | b'%'
                    )
                {
                    i += 1;
                }
                out.push_str(&input[start..i]);
            }
        }
    }
    out
}

/// Find the index of the `}` that balances the `{` at `open_after`. Scans
/// without recursing through escaped braces. Returns `input.len()` if no
/// match is found (unterminated group).
fn find_matching_brace(input: &str, open_after: usize) -> usize {
    let bytes = input.as_bytes();
    let mut depth = 1;
    let mut i = open_after;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return i - 1;
                }
            }
            _ => i += 1,
        }
    }
    input.len()
}

/// After `i` points just past `^`/`_`, advance past one char or one balanced
/// group — the subscript/superscript payload.
fn skip_script_payload(input: &str, i: usize) -> usize {
    let bytes = input.as_bytes();
    if i >= bytes.len() {
        return i;
    }
    if bytes[i] == b'{' {
        let end = find_matching_brace(input, i + 1);
        if end < input.len() {
            end + 1
        } else {
            input.len()
        }
    } else {
        i + 1
    }
}

/// Map subscript/superscript payload chars into their Unicode forms and
/// append to `out`. When any char is unmappable, falls back to `(...)` for
/// clarity.
fn parse_script(input: &str, i: usize, out: &mut String, sup: bool) {
    let bytes = input.as_bytes();
    if i >= bytes.len() {
        return;
    }
    let payload = if bytes[i] == b'{' {
        let end = find_matching_brace(input, i + 1);
        &input[i + 1..end.min(input.len())]
    } else {
        // Single char as a 1-byte slice.
        let start = i;
        let end = (i + 1).min(bytes.len());
        &input[start..end]
    };
    let table = if sup {
        &SUPERSCRIPT_TABLE
    } else {
        &SUBSCRIPT_TABLE
    };
    let mut mapped = String::new();
    let mut all_ok = true;
    for ch in payload.chars() {
        match table.get(&ch) {
            Some(&g) => mapped.push_str(g),
            None => {
                all_ok = false;
                break;
            }
        }
    }
    if all_ok && !mapped.is_empty() {
        out.push_str(&mapped);
    } else {
        // Unmappable char: emit the literal script syntax so it stays readable.
        out.push(if sup { '^' } else { '_' });
        out.push('(');
        out.push_str(payload);
        out.push(')');
    }
}

/// `\hat{x}` / `\bar y` style: read a single-char or braced argument, append
/// `base + combining` for each base char to `out`, return the new index.
fn parse_named_accent(input: &str, mut i: usize, out: &mut String, combining: &str) -> usize {
    let bytes = input.as_bytes();
    // Optional spaces before arg.
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    if i >= bytes.len() {
        return i;
    }
    if bytes[i] == b'{' {
        let end = find_matching_brace(input, i + 1);
        let inner = &input[i + 1..end.min(input.len())];
        for ch in inner.chars() {
            out.push(ch);
            out.push_str(combining);
        }
        if end < input.len() {
            end + 1
        } else {
            input.len()
        }
    } else {
        let ch = bytes[i] as char;
        out.push(ch);
        out.push_str(combining);
        i + 1
    }
}

/// `\frac{a}{b}`: render as `(a)/(b)`. Nested groups recurse through
/// `latex_to_unicode`.
fn parse_frac(input: &str, mut i: usize, out: &mut String) -> usize {
    let (num, ni) = read_group_or_char(input, i);
    out.push('(');
    out.push_str(&latex_to_unicode(&num));
    out.push(')');
    out.push('/');
    i = ni;
    let (den, ni) = read_group_or_char(input, i);
    out.push('(');
    out.push_str(&latex_to_unicode(&den));
    out.push(')');
    ni
}

/// `\sqrt{x}`: render as `√(x)` for groups, `√x` for a single char.
fn parse_sqrt(input: &str, mut i: usize, out: &mut String) -> usize {
    let bytes = input.as_bytes();
    // Optional [n] (root degree) — skip it.
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'[' {
        let close = find_byte(bytes, i + 1, b']');
        i = if close < bytes.len() {
            close + 1
        } else {
            bytes.len()
        };
    }
    let (payload, ni) = read_group_or_char(input, i);
    if payload.chars().count() > 1 {
        out.push('√');
        out.push('(');
        out.push_str(&latex_to_unicode(&payload));
        out.push(')');
    } else {
        out.push('√');
        out.push_str(&latex_to_unicode(&payload));
    }
    ni
}

/// Find the first occurrence of `target` at or after `from`. Returns
/// `bytes.len()` when not found.
fn find_byte(bytes: &[u8], from: usize, target: u8) -> usize {
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == target {
            return i;
        }
        i += 1;
    }
    bytes.len()
}

/// Read either a `{...}` group or a single char at `i`, returning the inner
/// text (without braces) and the index just past what was consumed.
fn read_group_or_char(input: &str, i: usize) -> (String, usize) {
    let bytes = input.as_bytes();
    if i >= bytes.len() {
        return (String::new(), i);
    }
    if bytes[i] == b'{' {
        let end = find_matching_brace(input, i + 1);
        let inner = &input[i + 1..end.min(input.len())];
        let next = if end < input.len() {
            end + 1
        } else {
            input.len()
        };
        (inner.to_string(), next)
    } else {
        let ch = (bytes[i] as char).to_string();
        (ch, i + 1)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn greek_lowercase() {
        assert_eq!(latex_to_unicode("\\alpha + \\beta"), "α + β");
        assert_eq!(
            latex_to_unicode("\\pi \\theta \\lambda \\mu \\sigma"),
            "π θ λ μ σ"
        );
        assert_eq!(latex_to_unicode("\\omega \\psi \\phi \\chi"), "ω ψ ϕ χ");
    }

    #[test]
    fn greek_uppercase() {
        assert_eq!(latex_to_unicode("\\Delta \\Sigma \\Omega"), "Δ Σ Ω");
        assert_eq!(latex_to_unicode("\\Gamma \\Lambda \\Pi"), "Γ Λ Π");
    }

    #[test]
    fn math_operators() {
        assert_eq!(latex_to_unicode("\\times"), "×");
        assert_eq!(latex_to_unicode("\\div"), "÷");
        assert_eq!(latex_to_unicode("\\pm \\mp"), "± ∓");
        assert_eq!(latex_to_unicode("\\cdot"), "⋅");
        assert_eq!(latex_to_unicode("\\sum \\prod \\int"), "∑ ∏ ∫");
    }

    #[test]
    fn relations_and_comparisons() {
        assert_eq!(latex_to_unicode("\\leq \\geq"), "≤ ≥");
        assert_eq!(latex_to_unicode("\\neq \\approx \\equiv"), "≠ ≈ ≡");
        assert_eq!(latex_to_unicode("\\infty \\partial \\nabla"), "∞ ∂ ∇");
    }

    #[test]
    fn arrows() {
        assert_eq!(latex_to_unicode("\\rightarrow \\leftarrow"), "→ ←");
        assert_eq!(latex_to_unicode("\\Rightarrow \\Leftarrow"), "⇒ ⇐");
        assert_eq!(
            latex_to_unicode("\\leftrightarrow \\uparrow \\downarrow"),
            "↔ ↑ ↓"
        );
    }

    #[test]
    fn sets_and_membership() {
        assert_eq!(latex_to_unicode("\\in \\notin"), "∈ ∉");
        assert_eq!(latex_to_unicode("\\subset \\supset"), "⊂ ⊃");
        assert_eq!(latex_to_unicode("\\cup \\cap"), "∪ ∩");
        assert_eq!(latex_to_unicode("\\emptyset"), "∅");
    }

    #[test]
    fn subscripts_single_digit() {
        assert_eq!(latex_to_unicode("x_1"), "x₁");
        assert_eq!(latex_to_unicode("x_2"), "x₂");
        assert_eq!(latex_to_unicode("x_0"), "x₀");
        assert_eq!(latex_to_unicode("x_9"), "x₉");
    }

    #[test]
    fn subscripts_group() {
        assert_eq!(latex_to_unicode("x_{10}"), "x₁₀");
        assert_eq!(latex_to_unicode("a_{ij}"), "aᵢⱼ");
        assert_eq!(latex_to_unicode("y_{n+1}"), "yₙ₊₁");
    }

    #[test]
    fn superscripts_single() {
        assert_eq!(latex_to_unicode("x^2"), "x²");
        assert_eq!(latex_to_unicode("x^3"), "x³");
        assert_eq!(latex_to_unicode("x^0"), "x⁰");
        assert_eq!(latex_to_unicode("x^+"), "x⁺");
        assert_eq!(latex_to_unicode("x^-"), "x⁻");
    }

    #[test]
    fn superscripts_group() {
        assert_eq!(latex_to_unicode("x^{n+1}"), "xⁿ⁺¹");
        assert_eq!(latex_to_unicode("e^{-1}"), "e⁻¹");
    }
    #[test]
    fn accents() {
        // Single-char escape accents — precomposed forms.
        assert_eq!(latex_to_unicode("\\'e"), "é");
        assert_eq!(latex_to_unicode("\\`e"), "è");
        assert_eq!(latex_to_unicode("\\^e"), "ê");
        assert_eq!(latex_to_unicode("\\\"e"), "ë");
        assert_eq!(latex_to_unicode("\\~n"), "ñ");
        // Single-char accents on braced groups.
        assert_eq!(latex_to_unicode("\\'{e}"), "é");
        assert_eq!(latex_to_unicode("\\^{ou}"), "ôû");
        // Named accents: \hat, \bar — emit decomposed (base + combining).
        assert_eq!(latex_to_unicode("\\hat e"), "e\u{0302}");
        assert_eq!(latex_to_unicode("\\bar y"), "y\u{0304}");
    }

    #[test]
    fn common_symbols() {
        assert_eq!(latex_to_unicode("\\degree"), "°");
        assert_eq!(latex_to_unicode("\\checkmark"), "✓");
        assert_eq!(latex_to_unicode("\\bullet"), "•");
        assert_eq!(latex_to_unicode("\\dots"), "…");
    }
    #[test]
    fn multi_replacement() {
        // Mixed symbols + scripts in one pass.
        assert_eq!(
            latex_to_unicode("\\alpha^2 + \\beta_1 \\leq \\gamma"),
            "α² + β₁ ≤ γ"
        );
    }

    #[test]
    fn nested_braces() {
        assert_eq!(latex_to_unicode("\\frac{a}{b}"), "(a)/(b)");
        assert_eq!(latex_to_unicode("\\sqrt{x+1}"), "√(x+1)");
        // Nested groups: {\alpha\beta} → αβ
        assert_eq!(latex_to_unicode("{\\alpha\\beta}"), "αβ");
    }

    #[test]
    fn no_match_passthrough() {
        // Plain text with no LaTeX is preserved.
        assert_eq!(latex_to_unicode("hello world"), "hello world");
        // Unknown commands emit their bare name.
        assert_eq!(latex_to_unicode("\\unknown"), "unknown");
        // Math delimiters are dropped in inline mode.
        assert_eq!(latex_to_unicode("$x$"), "x");
    }

    #[test]
    fn function_names_verbatim() {
        assert_eq!(latex_to_unicode("\\sin(x)"), "sin(x)");
        assert_eq!(latex_to_unicode("\\cos\\theta"), "cosθ");
    }

    #[test]
    fn has_latex_detects_commands() {
        assert!(has_latex("\\alpha"));
        assert!(has_latex("x^2"));
        assert!(has_latex("x_1"));
        assert!(has_latex("\\frac{a}{b}"));
        assert!(!has_latex("plain text"));
        assert!(!has_latex(""));
        assert!(!has_latex("no latex here"));
    }

    #[test]
    fn empty_input() {
        assert_eq!(latex_to_unicode(""), "");
        assert!(!has_latex(""));
    }
}

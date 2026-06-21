//! Minimal LaTeX → Unicode conversion for inline/display math.
//!
//! Markdown content may contain math delimited by `$...$` (inline) or
//! `$$...$$` (display). pulldown-cmark treats those as plain text, so without
//! this pass the model's `$\alpha + \beta = x^2$` renders verbatim. This module
//! scans for delimited math runs and rewrites common LaTeX commands,
//! superscripts, and subscripts into their Unicode equivalents, leaving all
//! other text (including stray `$` and code) untouched.
//!
//! Adapted from omp's `latex-to-unicode.ts`, scoped to the subset that covers
//! the overwhelming majority of math an LLM emits: Greek letters, common
//! operators/relations, and `^`/`_` scripts over digits and the letters that
//! have Unicode super/subscript forms. Anything unmapped falls back to the
//! literal source (`^q` stays `^q`), so output is always readable.

/// Single-char superscript forms, where Unicode provides one.
const SUPERSCRIPTS: &[(char, char)] = &[
    ('0', '⁰'),
    ('1', '¹'),
    ('2', '²'),
    ('3', '³'),
    ('4', '⁴'),
    ('5', '⁵'),
    ('6', '⁶'),
    ('7', '⁷'),
    ('8', '⁸'),
    ('9', '⁹'),
    ('+', '⁺'),
    ('-', '⁻'),
    ('=', '⁼'),
    ('(', '⁽'),
    (')', '⁾'),
    ('n', 'ⁿ'),
    ('i', 'ⁱ'),
    ('a', 'ᵃ'),
    ('b', 'ᵇ'),
    ('c', 'ᶜ'),
    ('d', 'ᵈ'),
    ('e', 'ᵉ'),
    ('f', 'ᶠ'),
    ('g', 'ᵍ'),
    ('h', 'ʰ'),
    ('j', 'ʲ'),
    ('k', 'ᵏ'),
    ('l', 'ˡ'),
    ('m', 'ᵐ'),
    ('o', 'ᵒ'),
    ('p', 'ᵖ'),
    ('r', 'ʳ'),
    ('s', 'ˢ'),
    ('t', 'ᵗ'),
    ('u', 'ᵘ'),
    ('v', 'ᵛ'),
    ('w', 'ʷ'),
    ('x', 'ˣ'),
    ('y', 'ʸ'),
    ('z', 'ᶻ'),
];

/// Single-char subscript forms, where Unicode provides one.
const SUBSCRIPTS: &[(char, char)] = &[
    ('0', '₀'),
    ('1', '₁'),
    ('2', '₂'),
    ('3', '₃'),
    ('4', '₄'),
    ('5', '₅'),
    ('6', '₆'),
    ('7', '₇'),
    ('8', '₈'),
    ('9', '₉'),
    ('+', '₊'),
    ('-', '₋'),
    ('=', '₌'),
    ('(', '₍'),
    (')', '₎'),
    ('a', 'ₐ'),
    ('e', 'ₑ'),
    ('h', 'ₕ'),
    ('i', 'ᵢ'),
    ('j', 'ⱼ'),
    ('k', 'ₖ'),
    ('l', 'ₗ'),
    ('m', 'ₘ'),
    ('n', 'ₙ'),
    ('o', 'ₒ'),
    ('p', 'ₚ'),
    ('r', 'ᵣ'),
    ('s', 'ₛ'),
    ('t', 'ₜ'),
    ('u', 'ᵤ'),
    ('v', 'ᵥ'),
    ('x', 'ₓ'),
];

/// `\name` LaTeX commands → Unicode. The leading backslash is implicit.
const COMMANDS: &[(&str, &str)] = &[
    // Greek lowercase
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
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
    ("pi", "π"),
    ("varpi", "ϖ"),
    ("rho", "ρ"),
    ("varrho", "ϱ"),
    ("sigma", "σ"),
    ("varsigma", "ς"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "φ"),
    ("varphi", "ϕ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    // Greek uppercase
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Upsilon", "Υ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    // Operators / relations / symbols
    ("times", "×"),
    ("div", "÷"),
    ("pm", "±"),
    ("mp", "∓"),
    ("cdot", "·"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("approx", "≈"),
    ("equiv", "≡"),
    ("propto", "∝"),
    ("infty", "∞"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("sum", "∑"),
    ("prod", "∏"),
    ("int", "∫"),
    ("oint", "∮"),
    ("sqrt", "√"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("neg", "¬"),
    ("in", "∈"),
    ("notin", "∉"),
    ("subset", "⊂"),
    ("subseteq", "⊆"),
    ("supset", "⊃"),
    ("cup", "∪"),
    ("cap", "∩"),
    ("emptyset", "∅"),
    ("rightarrow", "→"),
    ("to", "→"),
    ("leftarrow", "←"),
    ("Rightarrow", "⇒"),
    ("Leftarrow", "⇐"),
    ("leftrightarrow", "↔"),
    ("Leftrightarrow", "⇔"),
    ("mapsto", "↦"),
    ("circ", "∘"),
    ("ast", "∗"),
    ("star", "⋆"),
    ("bullet", "•"),
    ("ldots", "…"),
    ("cdots", "⋯"),
    ("degree", "°"),
    ("dagger", "†"),
    ("ddagger", "‡"),
];

fn script_char(table: &[(char, char)], c: char) -> Option<char> {
    table.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

/// Convert the body of a single math run (the text between the `$` delimiters)
/// from LaTeX to Unicode.
fn convert_math(latex: &str) -> String {
    let chars: Vec<char> = latex.chars().collect();
    let mut out = String::with_capacity(latex.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            // Read the longest alphabetic command name following the backslash.
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && chars[end].is_ascii_alphabetic() {
                end += 1;
            }
            if end > start {
                let name: String = chars[start..end].iter().collect();
                if let Some((_, repl)) = COMMANDS.iter().find(|(k, _)| *k == name) {
                    out.push_str(repl);
                    i = end;
                    continue;
                }
            } else if end < chars.len() {
                // `\` followed by a single non-letter (e.g. `\,`, `\%`): emit the
                // escaped char and move on.
                out.push(chars[end]);
                i = end + 1;
                continue;
            }
            // Unknown command: emit verbatim.
            out.push('\\');
            i += 1;
        } else if c == '^' || c == '_' {
            let table = if c == '^' { SUPERSCRIPTS } else { SUBSCRIPTS };
            if let Some((body, next)) = read_group(&chars, i + 1) {
                // Resolve LaTeX commands inside the script body first, then
                // super/subscript each resulting char that has a Unicode form.
                let resolved = convert_math(&body);
                let any_script = resolved.chars().any(|ch| script_char(table, ch).is_some());
                // Drop the caret/underscore when at least one char scripts, OR
                // the body was a command (e.g. `^\infty` → ∞). Otherwise keep
                // the literal marker so plain non-scriptable text (`^q`) stays
                // readable.
                if any_script || body.contains('\\') {
                    for ch in resolved.chars() {
                        out.push(script_char(table, ch).unwrap_or(ch));
                    }
                } else {
                    out.push(c);
                    out.push_str(&resolved);
                }
                i = next;
            } else {
                // Bare `^`/`_` with no operand — emit literally.
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Read a `^{...}` / `_{...}` group or a single following char. Returns the
/// inner text (without braces), the index past the group, or `None` if there is
/// no operand.
fn read_group(chars: &[char], at: usize) -> Option<(String, usize)> {
    let c = *chars.get(at)?;
    if c == '{' {
        let mut depth = 1;
        let mut end = at + 1;
        let mut body = String::new();
        while end < chars.len() {
            let ch = chars[end];
            if ch == '{' {
                depth += 1;
                body.push(ch);
            } else if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    return Some((body, end + 1));
                }
                body.push(ch);
            } else {
                body.push(ch);
            }
            end += 1;
        }
        Some((body, end)) // unterminated group — consume the rest
    } else if c == '\\' {
        // A backslash command as the operand (e.g. `^\infty`): read `\` plus
        // the following command name as one unit.
        let mut end = at + 1;
        while end < chars.len() && chars[end].is_ascii_alphabetic() {
            end += 1;
        }
        let body: String = chars[at..end].iter().collect();
        Some((body, end))
    } else {
        Some((c.to_string(), at + 1))
    }
}

/// Scan `input` for `$...$` and `$$...$$` math runs and convert each to
/// Unicode. Non-math text is copied verbatim; a lone, unbalanced `$` is left
/// untouched. Fast path: if the input contains no `$`, it is returned
/// unchanged. This is what the markdown renderer calls.
pub(crate) fn latex_to_unicode_owned(input: &str) -> String {
    if !input.contains('$') {
        return input.to_string();
    }
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let n = chars.len();
    while i < n {
        // Detect a math run starting at `i` as a single option: display
        // `$$...$$` first, then inline `$...$` (only if the body is mathy, so
        // a `$5` price is left alone). `(body_start, end, closer_len)`.
        let run = if chars[i] == '$' && i + 1 < n && chars[i + 1] == '$' {
            find_delim(&chars, i + 2, "$$").map(|end| (i + 2, end, 2))
        } else if chars[i] == '$' {
            find_delim(&chars, i + 1, "$")
                .filter(|end| {
                    let body: String = chars[i + 1..*end].iter().collect();
                    is_mathy(&body)
                })
                .map(|end| (i + 1, end, 1))
        } else {
            None
        };
        if let Some((body_start, end, closer_len)) = run {
            let body: String = chars[body_start..end].iter().collect();
            out.push_str(&convert_math(&body));
            i = end + closer_len;
            continue;
        }
        // No math run here — copy one char verbatim.
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Heuristic: does this `$...$` body look like math (vs. currency/plain text)?
/// True when it contains a LaTeX command, a superscript, or a subscript marker.
fn is_mathy(body: &str) -> bool {
    body.contains('\\') || body.contains('^') || body.contains('_')
}

/// Find the next occurrence of `delim` (1–2 chars) in `chars` starting at
/// `from`, returning the index of its first char. Escaped delimiters (`\$`)
/// are skipped. `None` means the delimiter was never closed.
fn find_delim(chars: &[char], from: usize, delim: &str) -> Option<usize> {
    let dc: Vec<char> = delim.chars().collect();
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 2;
            continue;
        }
        if chars[i] == dc[0] {
            if dc.len() == 1 {
                return Some(i);
            }
            if i + 1 < chars.len() && chars[i + 1] == dc[1] {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_math_returns_input_unchanged() {
        assert_eq!(latex_to_unicode_owned("hello world"), "hello world");
        assert_eq!(
            latex_to_unicode_owned("cost is $5 and $10"),
            "cost is $5 and $10"
        );
    }

    #[test]
    fn lone_unbalanced_dollar_left_untouched() {
        assert_eq!(latex_to_unicode_owned("price: $5"), "price: $5");
        assert_eq!(latex_to_unicode_owned("a $ b"), "a $ b");
    }

    #[test]
    fn inline_greek_and_operators() {
        assert_eq!(latex_to_unicode_owned(r"$\alpha + \beta$"), "α + β");
        assert_eq!(latex_to_unicode_owned(r"$\sum \times \infty$"), "∑ × ∞");
    }

    #[test]
    fn superscripts_and_subscripts() {
        assert_eq!(latex_to_unicode_owned(r"$x^2 + y^2$"), "x² + y²");
        assert_eq!(latex_to_unicode_owned(r"$a_{ij}$"), "aᵢⱼ");
        assert_eq!(latex_to_unicode_owned(r"$2^n$"), "2ⁿ");
    }

    #[test]
    fn superscript_without_unicode_form_falls_back() {
        // 'q' has no superscript form — keep the literal `^q`.
        assert_eq!(latex_to_unicode_owned(r"$x^q$"), "x^q");
    }

    #[test]
    fn display_math_block() {
        assert_eq!(
            latex_to_unicode_owned(r"$$\int_0^\infty e^{-x} dx$$"),
            "∫₀∞ e⁻ˣ dx"
        );
    }

    #[test]
    fn mixed_text_and_math() {
        assert_eq!(
            latex_to_unicode_owned(r"Euler: $e^{i\pi} + 1 = 0$ holds."),
            "Euler: eⁱπ + 1 = 0 holds."
        );
    }

    #[test]
    fn escaped_dollar_inside_math() {
        // `\$` inside math is skipped as a delimiter candidate; the run closes
        // at the real trailing `$`.
        assert_eq!(latex_to_unicode_owned(r"cost $\$5$"), "cost $5");
    }

    #[test]
    fn unknown_command_emitted_verbatim() {
        assert_eq!(latex_to_unicode_owned(r"$\foobar x$"), r"\foobar x");
    }
}

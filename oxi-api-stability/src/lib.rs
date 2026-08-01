// oxi-api-stability/src/lib.rs
//! Stability tier attribute macros for the oxi workspace.
//!
//! Provides four attributes that render as colored badges in `cargo doc`:
//! - `#[stable(since = "0.63.0")]` — green badge, semver-stable
//! - `#[unstable(feature = "browser")]` — amber badge, may change
//! - `#[internal]` — hides from docs (`#[doc(hidden)]`)
//! - `#[deprecated(since = "0.64.0")]` — red badge + native deprecation warning

use proc_macro::TokenStream;
use quote::quote;
use syn::{MetaNameValue, parse2};

/// Parsed `since = "0.XX.0"` argument shared by `#[stable]` and `#[deprecated]`.
#[derive(Debug)]
struct SinceArg {
    since: String,
}
impl syn::parse::Parse for SinceArg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let nv: MetaNameValue = input.parse()?;
        if !matches!(nv.path.get_ident(), Some(id) if id == "since") {
            return Err(syn::Error::new_spanned(
                &nv.path,
                "expected `since = \"...\"`",
            ));
        }
        let since = match nv.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => s.value(),
            other => return Err(syn::Error::new_spanned(other, "expected string literal")),
        };
        Ok(Self { since })
    }
}

/// Parsed `feature = "name"` argument for `#[unstable]`.
#[derive(Debug)]
struct FeatureArg {
    feature: String,
}
impl syn::parse::Parse for FeatureArg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let nv: MetaNameValue = input.parse()?;
        if !matches!(nv.path.get_ident(), Some(id) if id == "feature") {
            return Err(syn::Error::new_spanned(
                &nv.path,
                "expected `feature = \"...\"`",
            ));
        }
        let feature = match nv.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => s.value(),
            other => return Err(syn::Error::new_spanned(other, "expected string literal")),
        };
        Ok(Self { feature })
    }
}

/// Parsed `since = "0.XX.0", note = "..."` for `#[deprecated(...)]`.
/// The note is optional (matches the native `#[deprecated]` behavior).
#[derive(Debug)]
struct DeprecArg {
    since: String,
    note: Option<String>,
}
impl syn::parse::Parse for DeprecArg {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut since: Option<String> = None;
        let mut note: Option<String> = None;
        // Accept: `since = "...", note = "..."` (note is optional, order-independent).
        while !input.is_empty() {
            let nv: MetaNameValue = input.parse()?;
            let value = match nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => s.value(),
                other => return Err(syn::Error::new_spanned(other, "expected string literal")),
            };
            match nv.path.get_ident().map(|i| i.to_string()).as_deref() {
                Some("since") => since = Some(value),
                Some("note") => note = Some(value),
                _ => {
                    return Err(syn::Error::new_spanned(
                        &nv.path,
                        "expected `since` or `note`",
                    ));
                }
            }
            if input.peek(syn::Token![,]) {
                let _: syn::Token![,] = input.parse()?;
            }
        }
        let since = since.ok_or_else(|| input.error("missing `since = \"...\"`"))?;
        Ok(Self { since, note })
    }
}

/// Attribute that marks an item as semver-stable.
///
/// The macro name shadows the built-in `#[stable]` (rustdoc-only)
/// inside the same use-scope — use the qualified path
/// `#[oxi_api_stability::stable(...)]` if you also need the built-in.
#[proc_macro_attribute]
pub fn stable(args: TokenStream, input: TokenStream) -> TokenStream {
    let parsed = match parse2::<SinceArg>(args.into()) {
        Ok(p) => p,
        Err(e) => return e.to_compile_error().into(),
    };
    let since_val = parsed.since;
    let input: proc_macro2::TokenStream = input.into();
    quote! {
        #[doc = concat!(" <div class=\"stab stable\"><strong>Stable</strong> since ", #since_val, "</div>")]
        #input
    }
    .into()
}

/// Attribute that marks an item as semver-unstable.
///
/// The macro name shadows the built-in `#[unstable]` (rustdoc-only)
/// inside the same use-scope — use the qualified path
/// `#[oxi_api_stability::unstable(...)]` if you also need the built-in.
#[proc_macro_attribute]
pub fn unstable(args: TokenStream, input: TokenStream) -> TokenStream {
    let parsed = match parse2::<FeatureArg>(args.into()) {
        Ok(p) => p,
        Err(e) => return e.to_compile_error().into(),
    };
    let feature_val = parsed.feature;
    let input: proc_macro2::TokenStream = input.into();
    quote! {
        #[doc = concat!(" <div class=\"stab unstable\"><strong>Unstable</strong> (feature: ", #feature_val, ") — may change or be removed</div>")]
        #input
    }
    .into()
}

/// Attribute that hides an item from consumer-facing docs.
#[proc_macro_attribute]
pub fn internal(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input: proc_macro2::TokenStream = input.into();
    quote! {
        #[doc(hidden)]
        #input
    }
    .into()
}

/// Attribute that marks an item as deprecated with a doc badge.
/// Emits the native `#[deprecated(since=..., note=...)]` so consumers don't
/// need a second attribute. The macro name shadows the built-in `#[deprecated]`
/// only inside the same use-scope — emit it via the qualified path
/// `#[oxi_api_stability::deprecated(...)]` or by importing under a different
/// name (`use oxi_api_stability::deprecated as oxi_deprecated;`) if you also
/// need the built-in.
/// Accepts: `since = "0.XX.0"` (required), `note = "..."` (optional).
#[proc_macro_attribute]
pub fn deprecated(args: TokenStream, input: TokenStream) -> TokenStream {
    let parsed = match parse2::<DeprecArg>(args.into()) {
        Ok(p) => p,
        Err(e) => return e.to_compile_error().into(),
    };
    let since_val = parsed.since;
    let note_lit = parsed.note.as_deref().unwrap_or("");
    let input: proc_macro2::TokenStream = input.into();
    quote! {
        #[deprecated(since = #since_val, note = #note_lit)]
        #[doc = concat!(" <div class=\"stab deprecated\"><strong>Deprecated</strong> since ", #since_val, "</div>")]
        #input
    }
    .into()
}

#[cfg(test)]
mod tests {
    // The proc-macro entry points cannot be exercised on test items in this
    // crate -- `#[stable]` is rejected with E0734 outside the standard library,
    // and `#[unstable]` / `#[deprecated]` shadow stdlib built-ins (E0659). The
    // end-to-end usage is covered by downstream integration tests in
    // oxi-sdk / oxi-cli (Phase 2 Task 6+).
    //
    // Here we unit-test the parser behavior, which is the only piece testable
    // from inside the crate. The macro entry points compile because
    // `cargo build -p oxi-api-stability` succeeds.

    use super::{DeprecArg, FeatureArg, SinceArg};
    use proc_macro2::TokenStream;
    use syn::parse2;

    fn ts(s: &str) -> TokenStream {
        s.parse().expect("valid token stream")
    }

    #[test]
    fn since_arg_parses() {
        let arg: SinceArg = parse2(ts(r#"since = "0.63.0""#)).unwrap();
        assert_eq!(arg.since, "0.63.0");
    }

    #[test]
    fn since_arg_rejects_wrong_key() {
        let res: syn::Result<SinceArg> = parse2(ts(r#"feature = "browser""#));
        assert!(res.is_err(), "expected error for wrong key");
    }

    #[test]
    fn feature_arg_parses() {
        let arg: FeatureArg = parse2(ts(r#"feature = "browser""#)).unwrap();
        assert_eq!(arg.feature, "browser");
    }

    #[test]
    fn feature_arg_rejects_wrong_key() {
        let res: syn::Result<FeatureArg> = parse2(ts(r#"since = "0.63.0""#));
        assert!(res.is_err(), "expected error for wrong key");
    }

    #[test]
    fn deprec_arg_only_since() {
        let arg: DeprecArg = parse2(ts(r#"since = "0.64.0""#)).unwrap();
        assert_eq!(arg.since, "0.64.0");
        assert_eq!(arg.note, None);
    }

    #[test]
    fn deprec_arg_since_and_note() {
        let arg: DeprecArg = parse2(ts(r#"since = "0.64.0", note = "use new api""#)).unwrap();
        assert_eq!(arg.since, "0.64.0");
        assert_eq!(arg.note.as_deref(), Some("use new api"));
    }

    #[test]
    fn deprec_arg_note_first_order_independent() {
        let arg: DeprecArg = parse2(ts(r#"note = "see docs", since = "0.64.0""#)).unwrap();
        assert_eq!(arg.since, "0.64.0");
        assert_eq!(arg.note.as_deref(), Some("see docs"));
    }

    #[test]
    fn deprec_arg_missing_since_errors() {
        let res: syn::Result<DeprecArg> = parse2(ts(r#"note = "no since""#));
        assert!(res.is_err(), "expected error for missing since");
    }

    #[test]
    fn deprec_arg_rejects_unknown_key() {
        let res: syn::Result<DeprecArg> = parse2(ts(r#"reason = "nope""#));
        assert!(res.is_err(), "expected error for unknown key");
    }

    #[test]
    fn deprec_arg_rejects_non_string_value() {
        let res: syn::Result<DeprecArg> = parse2(ts(r#"since = 42"#));
        assert!(res.is_err(), "expected error for non-string value");
    }
}

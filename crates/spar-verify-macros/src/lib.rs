use proc_macro::TokenStream;
use quote::quote;
use syn::{Expr, Item, Lit, parse_macro_input};

/// Marks a module as containing AADL configuration constants.
///
/// When applied to a module, this attribute:
/// 1. Preserves the module unchanged (pass-through).
/// 2. Future versions will verify constants against the AADL model at build
///    time via `spar verify`.
///
/// # Example
///
/// ```rust,ignore
/// #[spar_verify::aadl_config]
/// pub mod ctrl {
///     pub const COMPONENT: &str = "SensorFusion::Ctrl.Impl";
///     pub const CATEGORY: &str = "thread";
///     pub const PERIOD_PS: u64 = 10_000_000_000;
/// }
/// ```
#[proc_macro_attribute]
pub fn aadl_config(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let module = parse_macro_input!(item as syn::ItemMod);

    // Extract COMPONENT const value if present — used for diagnostics.
    let component_name = module
        .content
        .as_ref()
        .and_then(|(_, items)| {
            items.iter().find_map(|item| {
                if let Item::Const(c) = item {
                    if c.ident == "COMPONENT" {
                        if let Expr::Lit(lit) = &*c.expr {
                            if let Lit::Str(s) = &lit.lit {
                                return Some(s.value());
                            }
                        }
                    }
                }
                None
            })
        })
        .unwrap_or_else(|| module.ident.to_string());

    let note = format!("AADL config for {component_name} — use `spar verify` to check");

    // Pass through unchanged — verification happens via `spar verify`.
    let expanded = quote! {
        #module

        // Emit a compile-time diagnostic so users know the attribute was
        // processed. This is a no-op at runtime.
        #[allow(unused)]
        const _: () = {
            #[deprecated(note = #note)]
            const _AADL_CONFIG_HINT: () = ();
        };
    };

    expanded.into()
}

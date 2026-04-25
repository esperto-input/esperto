use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use proc_macro_error2::proc_macro_error;
use quote::quote;
use syn::{ItemImpl, Path, parse2};

#[proc_macro_derive(Scalar)]
#[proc_macro_error]
pub fn derive_scalar(item: TokenStream) -> TokenStream {
   replace_scalar_trait(
      frozen_collections_core::macros::derive_scalar_macro(item.into())
         .unwrap_or_else(|error| error.to_compile_error()),
      syn::parse_str("::esperto::types::Scalar").unwrap(),
   )
   .unwrap_or_else(|error| error.to_compile_error())
   .into()
}

/// Replaces the trait path in a generated impl block.
fn replace_scalar_trait(generated_tokens: TokenStream2, new_trait_path: Path) -> Result<TokenStream2, syn::Error> {
   // 1. Parse the raw TokenStream into a syn::ItemImpl AST node
   let mut item_impl: ItemImpl = parse2(generated_tokens)?;

   // 2. item_impl.trait_ is an Option containing the tuple:
   //    (Option<Bang>, Path, ForToken)
   if let Some((_bang, ref mut trait_path, _for_token)) = item_impl.trait_ {
      // Optional robustness: Verify it's actually the path we expect before overwriting
      let path_string = quote!(#trait_path).to_string();
      // Note: quote drops whitespace, so it will look exactly like this
      if path_string == ":: frozen_collections :: Scalar" {
         // 3. Mutate the AST with the new path
         *trait_path = new_trait_path;
      }
   } else {
      return Err(syn::Error::new_spanned(
         &item_impl,
         "Expected an impl block that implements a trait",
      ));
   }

   // 4. Convert the mutated AST back to a TokenStream
   Ok(quote!(#item_impl))
}

// /// Implementation logic for the `Scalar` derive macro.
// ///
// /// # Errors
// ///
// /// Bad things happen to bad input
// fn derive_scalar_macro(args: TokenStream2, path: Path) -> syn::Result<TokenStream2> {
//    let input: DeriveInput = syn::parse2(args)?;
//    let name = &input.ident;
//    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
//
//    let Data::Enum(variants) = &input.data else {
//       return Err(Error::new_spanned(name, "Scalar can only be used with enums"));
//    };
//
//    if variants.variants.is_empty() {
//       return Err(Error::new_spanned(name, "Scalar can only be used with non-empty enums"));
//    }
//
//    for v in &variants.variants {
//       if v.fields != Fields::Unit {
//          return Err(Error::new_spanned(
//             &v.ident,
//             "Scalar can only be used with enums that only contain unit variants",
//          ));
//       }
//
//       if v.discriminant.is_some() {
//          return Err(Error::new_spanned(
//             &v.ident,
//             "Scalar can only be used with enums that do not have explicit discriminants",
//          ));
//       }
//    }
//
//    let mut matches = Vec::new();
//    for variant in &variants.variants {
//       let ident = &variant.ident;
//
//       let index = matches.len();
//       matches.push(quote! { #name::#ident => #index});
//    }
//
//    Ok(quote! {
//         #[automatically_derived]
//         impl #impl_generics #path for #name #ty_generics #where_clause {
//             fn index(&self) -> usize {
//                 match self {
//                     #(#matches),*
//                 }
//             }
//         }
//     })
// }

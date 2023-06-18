use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, spanned::Spanned, Data, DeriveInput, Error, Fields};

#[proc_macro_derive(EnumVariants)]
pub fn derive_named_variant(input: TokenStream) -> TokenStream {
	let input = parse_macro_input!(input as DeriveInput);

	let name = &input.ident;
	let data = &input.data;

	let stream: TokenStream2 = match data {
		Data::Enum(data_enum) => {
			let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

			let variant_arms = data_enum.variants.iter().map(|variant| {
				let ident = &variant.ident;
				let fields = match variant.fields {
					Fields::Named(_) => quote!({ .. }),
					Fields::Unnamed(_) => quote!((..)),
					Fields::Unit => quote!(),
				};
				let name = ident.to_string();
				quote!(Self::#ident #fields => #name)
			});
			let count = data_enum.variants.len();
			let names = data_enum.variants.iter().map(|variant| {
				let name = variant.ident.to_string();
				quote!(#name)
			});

			quote! {
				impl #impl_generics ::enum_variants::EnumVariants for #name #ty_generics #where_clause {
					fn name(&self) -> &'static str {
						match self {
							#(#variant_arms),*
						}
					}
					fn names() -> &'static [&'static str] {
						const NAMES: [&'static str; #count] = [
							#(#names),*
						];
						&NAMES
					}
				}
			}
			.into()
		}
		_ => Error::new(input.span(), "input should be enum").to_compile_error(),
	};
	stream.into()
}

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

use crate::types::{FieldIndex, LedgerField, TypeNode};

use super::helpers::make_ident;
use super::types::type_to_tokens;

pub(crate) fn emit_ledger_wrapper(fields: &[LedgerField], name: &str) -> TokenStream {
    let struct_name = format_ident!("{}", name);

    let accessors: Vec<_> = fields
        .iter()
        .filter_map(|field| {
            let field_index = field.field_index()?;
            let const_name = format_ident!("FIELD_{}", field.name.to_uppercase());
            Some(emit_field_accessor(field, &const_name, &field_index))
        })
        .collect();

    quote! {
        /// Typed read-only access to the contract's ledger state.
        pub struct #struct_name {
            state: ContractState<InMemoryDB>,
        }

        impl #struct_name {
            /// Create from a deserialized `ContractState`.
            pub fn new(state: ContractState<InMemoryDB>) -> Self {
                Self { state }
            }

            /// Create from a hex-encoded contract state string (as returned by the indexer).
            pub fn from_hex(hex_state: &str) -> Result<Self, StateError> {
                let bytes = hex::decode(hex_state).map_err(|e| StateError::HexDecode(e.to_string()))?;
                let state: ContractState<InMemoryDB> = tagged_deserialize(&mut &bytes[..]).map_err(StateError::Deserialize)?;
                Ok(Self::new(state))
            }

            #(#accessors)*
        }
    }
}

/// Generate a `get_field` or `get_field_path` call depending on the index type.
fn navigate_to_field(const_name: &Ident, field_index: &FieldIndex) -> TokenStream {
    match field_index {
        FieldIndex::Single(_) => {
            quote! { get_field(self.state.data.get_ref(), #const_name) }
        }
        FieldIndex::Path(_) => {
            quote! { get_field_path(self.state.data.get_ref(), #const_name) }
        }
    }
}

fn emit_field_accessor(
    field: &LedgerField,
    const_name: &Ident,
    field_index: &FieldIndex,
) -> TokenStream {
    let method_name = make_ident(&field.name);
    let nav = navigate_to_field(const_name, field_index);
    let doc = format!(
        "Access the `{}` ledger field ({}).",
        field.name, field.storage
    );

    match field.storage.as_str() {
        "cell" => emit_cell_accessor(&method_name, &doc, &nav, field.cell_type.as_ref()),
        "counter" => emit_counter_accessor(&method_name, &doc, &nav),
        "map" => emit_map_accessor(&method_name, &doc, &nav, field),
        "set" => emit_set_accessor(&method_name, &doc, &nav, field),
        "list" => emit_list_accessor(&method_name, &doc, &nav, field),
        "merkle-tree" | "historic-merkle-tree" => {
            emit_merkle_tree_accessor(&method_name, &doc, &nav)
        }
        _ => {
            quote! {
                #[doc = #doc]
                pub fn #method_name(&self) -> Result<&StateValue<InMemoryDB>, StateError> {
                    #nav
                }
            }
        }
    }
}

fn emit_cell_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    cell_type: Option<&TypeNode>,
) -> TokenStream {
    if let Some(ty) = cell_type {
        let (ret_type, body) = cell_accessor(ty, nav);
        quote! {
            #[doc = #doc]
            pub fn #method_name(&self) -> Result<#ret_type, StateError> {
                #body
            }
        }
    } else {
        quote! {
            #[doc = #doc]
            pub fn #method_name(&self) -> Result<&StateValue<InMemoryDB>, StateError> {
                #nav
            }
        }
    }
}

fn emit_counter_accessor(method_name: &Ident, doc: &str, nav: &TokenStream) -> TokenStream {
    let body = cell_value_body(&quote! { u64 }, nav);
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<u64, StateError> {
            #body
        }
    }
}

fn emit_map_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let key_ty = field
        .key_type
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    let val_ty = field
        .value_type
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<MapAccessor<'_, #key_ty, #val_ty>, StateError> {
            let sv = #nav?;
            match sv {
                StateValue::Map(map) => Ok(MapAccessor::new(map)),
                _ => Err(StateError::UnexpectedVariant {
                    expected: "Map",
                    actual: variant_name(sv),
                }),
            }
        }
    }
}

fn emit_set_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let elem_ty = field
        .element_type
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<SetAccessor<'_, #elem_ty>, StateError> {
            let sv = #nav?;
            match sv {
                StateValue::Map(map) => Ok(SetAccessor::new(map)),
                _ => Err(StateError::UnexpectedVariant {
                    expected: "Set",
                    actual: variant_name(sv),
                }),
            }
        }
    }
}

fn emit_list_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let elem_ty = field
        .element_type
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<ListAccessor<'_, #elem_ty>, StateError> {
            let sv = #nav?;
            match sv {
                StateValue::Array(arr) => Ok(ListAccessor::new(arr)),
                _ => Err(StateError::UnexpectedVariant {
                    expected: "Array",
                    actual: variant_name(sv),
                }),
            }
        }
    }
}

fn emit_merkle_tree_accessor(method_name: &Ident, doc: &str, nav: &TokenStream) -> TokenStream {
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<MerkleTreeAccessor<'_>, StateError> {
            let sv = #nav?;
            MerkleTreeAccessor::from_state(sv)
        }
    }
}

fn cell_accessor(ty: &TypeNode, nav: &TokenStream) -> (TokenStream, TokenStream) {
    if let TypeNode::Alias { inner, .. } = ty {
        cell_accessor(inner, nav)
    } else {
        let ret_type = type_to_tokens(ty);
        let body = cell_value_body(&ret_type, nav);
        (ret_type, body)
    }
}

/// Generate the body for a cell accessor that uses `cell_value` + `TryFrom<&ValueSlice>`.
fn cell_value_body(ret_type: &TokenStream, nav: &TokenStream) -> TokenStream {
    quote! {
        let sv = #nav?;
        let av = cell_value(sv)?;
        <#ret_type>::try_from(&*av.value).map_err(StateError::Conversion)
    }
}

use crate::contract::{methods, Context};
use crate::util;
use anyhow::{Context as _, Result};
use ethcontract_common::Address;
use inflector::Inflector;
use proc_macro2::{Literal, TokenStream};
use quote::quote;

pub(crate) fn expand(cx: &Context) -> Result<TokenStream> {
    let deployed = expand_deployed(&cx);
    let deploy =
        expand_deploy(&cx).context("error generating contract `deploy` associated function")?;

    Ok(quote! {
        #deployed
        #deploy
    })
}

fn expand_deployed(cx: &Context) -> TokenStream {
    if cx.artifact.networks.is_empty() && cx.deployments.is_empty() {
        return quote! {};
    }

    let artifact_network = quote! { artifact.networks.get(network_id)?  };
    let network = if cx.deployments.is_empty() {
        artifact_network
    } else {
        let deployments = cx.deployments.iter().map(|(network_id, address)| {
            let network_id = Literal::string(&network_id.to_string());
            let address = expand_address(*address);

            quote! {
                #network_id => self::ethcontract::common::truffle::Network {
                    address: #address,
                    transaction_hash: None,
                },
            }
        });

        quote! {
            match network_id {
                #( #deployments )*
                _ => #artifact_network.clone(),
            };
        }
    };

    quote! {
        impl Contract {
            /// Locates a deployed contract based on the current network ID
            /// reported by the `web3` provider.
            ///
            /// Note that this does not verify that a contract with a maching
            /// `Abi` is actually deployed at the given address.
            pub fn deployed<F, T>(
                web3: &self::ethcontract::web3::api::Web3<T>,
            ) -> self::ethcontract::dyns::DynDeployedFuture<Self>
            where
                F: self::ethcontract::web3::futures::Future<
                    Item = self::ethcontract::json::Value,
                    Error = self::ethcontract::web3::Error
                > + Send + 'static,
                T: self::ethcontract::web3::Transport<Out = F> + Send + Sync + 'static,
            {
                use self::ethcontract::contract::DeployedFuture;
                use self::ethcontract::transport::DynTransport;
                use self::ethcontract::web3::api::Web3;

                let transport = DynTransport::new(web3.transport().clone());
                let web3 = Web3::new(transport);

                DeployedFuture::new(web3, ())
            }
        }

        impl self::ethcontract::contract::FromNetwork<self::ethcontract::dyns::DynTransport>
            for Contract
        {
            type Context = ();

            fn from_network(
                web3: self::ethcontract::dyns::DynWeb3,
                network_id: &str,
                _: Self::Context,
            ) -> Option<Self> {
                let artifact = Self::artifact();
                let network = #network;

                Some(Self::with_transaction(
                    &web3,
                    network.address,
                    network.transaction_hash,
                ))
            }
        }
    }
}

fn expand_deploy(cx: &Context) -> Result<TokenStream> {
    if cx.artifact.bytecode.is_empty() {
        // do not generate deploy method for contracts that have empty bytecode
        return Ok(quote! {});
    }

    // TODO(nlordell): not sure how contructor documentation get generated as I
    //   can't seem to get truffle to output it
    let doc = util::expand_doc("Generated by `ethcontract`");

    let (input, arg) = match cx.artifact.abi.constructor() {
        Some(contructor) => (
            methods::expand_inputs(&contructor.inputs)?,
            methods::expand_inputs_call_arg(&contructor.inputs),
        ),
        None => (quote! {}, quote! {()}),
    };

    let libs: Vec<_> = cx
        .artifact
        .bytecode
        .undefined_libraries()
        .map(|name| (name, util::safe_ident(&name.to_snake_case())))
        .collect();
    let (lib_struct, lib_input, link) = if !libs.is_empty() {
        let lib_struct = {
            let lib_struct_fields = libs.iter().map(|(name, field)| {
                let doc = util::expand_doc(&format!("Address of the `{}` library.", name));

                quote! {
                    #doc pub #field: self::ethcontract::Address
                }
            });

            quote! {
                /// Undefinied libraries in the contract bytecode that are
                /// required for linking in order to deploy.
                pub struct Libraries {
                    #( #lib_struct_fields, )*
                }
            }
        };

        let link = {
            let link_libraries = libs.iter().map(|(name, field)| {
                let name_lit = Literal::string(&name);

                quote! {
                    bytecode.link(#name_lit, libs.#field).expect("valid library");
                }
            });

            quote! {
                let mut bytecode = bytecode;
                #( #link_libraries )*
            }
        };

        (lib_struct, quote! { , libs: Libraries }, link)
    } else {
        Default::default()
    };

    Ok(quote! {
        #lib_struct

        impl Contract {
            #doc
            pub fn builder<F, T>(
                web3: &self::ethcontract::web3::api::Web3<T> #lib_input #input ,
            ) -> self::ethcontract::dyns::DynDeployBuilder<Self>
            where
                F: self::ethcontract::web3::futures::Future<
                    Item = self::ethcontract::json::Value,
                    Error = self::ethcontract::web3::Error,
                > + Send + 'static,
                T: self::ethcontract::web3::Transport<Out = F> + Send + Sync + 'static,
            {
                use self::ethcontract::dyns::DynTransport;
                use self::ethcontract::contract::DeployBuilder;
                use self::ethcontract::web3::api::Web3;

                let transport = DynTransport::new(web3.transport().clone());
                let web3 = Web3::new(transport);

                let bytecode = Self::artifact().bytecode.clone();
                #link

                DeployBuilder::new(web3, bytecode, #arg).expect("valid deployment args")
            }
        }

        impl self::ethcontract::contract::Deploy<self::ethcontract::dyns::DynTransport> for Contract {
            type Context = self::ethcontract::common::Bytecode;

            fn bytecode(cx: &Self::Context) -> &self::ethcontract::common::Bytecode {
                cx
            }

            fn abi(_: &Self::Context) -> &self::ethcontract::common::Abi {
                &Self::artifact().abi
            }

            fn from_deployment(
                web3: self::ethcontract::dyns::DynWeb3,
                address: self::ethcontract::Address,
                transaction_hash: self::ethcontract::H256,
                _: Self::Context,
            ) -> Self {
                Self::with_transaction(&web3, address, Some(transaction_hash))
            }
        }
    })
}

/// Expands an `Address` into a literal representation that can be used with
/// quasi-quoting for code generation.
fn expand_address(address: Address) -> TokenStream {
    let bytes = address
        .as_bytes()
        .iter()
        .copied()
        .map(Literal::u8_unsuffixed);

    quote! {
        self::ethcontract::Address::from([#( #bytes ),*])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[rustfmt::skip]
    fn expand_address_value() {
        assert_quote!(
            expand_address(Address::zero()),
            {
                self::ethcontract::Address::from([ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 ])
            },
        );

        assert_quote!(
            expand_address("000102030405060708090a0b0c0d0e0f10111213".parse().unwrap()),
            {
                self::ethcontract::Address::from([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19])
            },
        );
    }
}

use std::fmt::Debug;

use proc_macro2::TokenStream;
use quote::{quote, TokenStreamExt};

use super::*;
use crate::{CodegenContext, LocalContext, MacroArgType};

/// A function call.
///
/// ```c
/// #define function_call f(1, 2, 3)
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionCall {
  /// The function name identifier.
  pub(crate) name: Identifier,
  /// The function arguments.
  pub(crate) args: Vec<Expr>,
}

impl FunctionCall {
  pub(crate) fn finish<'g, C>(&mut self, ctx: &mut LocalContext<'g, C>) -> Result<Option<Type>, crate::Error>
  where
    C: CodegenContext,
  {
    self.name.finish(ctx)?;

    for arg in self.args.iter_mut() {
      arg.finish(ctx)?;
    }

    let mut ty = None;

    if let Identifier::Literal(ref function_name) = self.name {
      if let Some((known_args, known_ret_ty)) = ctx.function(function_name.as_str()) {
        if known_args.len() == self.args.len() {
          if let Ok(y) = syn::parse_str::<syn::Type>(&known_ret_ty) {
            ty = Some(Type::try_from(y)?);
          }

          for (arg, known_arg_type) in self.args.iter_mut().zip(known_args.iter()) {
            arg.finish(ctx)?;

            // If the current argument to this function is a macro argument,
            // we can infer the type of the macro argument.
            if let Expr::Variable { name: Identifier::Literal(ref name) } = arg {
              if let Some(arg_type) = ctx.arg_type_mut(name) {
                if *arg_type == MacroArgType::Unknown {
                  if let Ok(ty) = syn::parse_str::<syn::Type>(known_arg_type) {
                    *arg_type = MacroArgType::Known(Type::try_from(ty)?);
                  }
                }
              }
            }
          }
        }
      }
    }

    for arg in self.args.iter_mut() {
      arg.finish(ctx)?;
    }

    // TODO: Get function return type from context.
    Ok(ty)
  }

  pub(crate) fn to_tokens<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, C>, tokens: &mut TokenStream) {
    let mut name = TokenStream::new();
    self.name.to_tokens(ctx, &mut name);

    let args = self.args.iter().map(|arg| {
      let into = matches!(arg, Expr::Variable { .. })
        && !matches!(arg, Expr::Variable { name: Identifier::Literal(id) } if id == "NULL");

      let arg = arg.to_token_stream(ctx);

      if into {
        return quote! { #arg.into() }
      }

      arg
    });

    tokens.append_all(quote! {
      #name(#(#args),*)
    })
  }
}
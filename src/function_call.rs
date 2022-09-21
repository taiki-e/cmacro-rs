use quote::TokenStreamExt;
use quote::quote;

use super::*;

/// A function call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCall {
  pub name: Identifier,
  pub args: Vec<Expr>,
}

impl FunctionCall {
  pub fn visit<'s, 't>(&mut self, ctx: &mut Context<'s, 't>) {
    self.name.visit(ctx);

    for arg in self.args.iter_mut() {
      arg.visit(ctx);
    }

    if let Identifier::Literal(ref function_name) = self.name {
      if let Some(known_args) = ctx.functions.get(function_name.as_str()).cloned() {
        if known_args.len() == self.args.len() {
          for (arg, known_arg_type) in self.args.iter_mut().zip(known_args.iter()) {
            // arg.visit(ctx);

            // If the current argument to this function is a macro argument,
            // we can infer the type of the macro argument.
            if let Expr::Variable { name: Identifier::Literal( ref name) } = arg {
              if let Some(arg_type) = ctx.arg_type_mut(name) {
                if *arg_type == MacroArgType::Unknown {
                  *arg_type = MacroArgType::Known(known_arg_type.clone());
                }
              }
            }
          }
        }
      }
    }

    for arg in self.args.iter_mut() {
      arg.visit(ctx);
    }
  }

  pub fn to_tokens(&self, ctx: &mut Context, tokens: &mut TokenStream) {
    let mut name = TokenStream::new();
    self.name.to_tokens(ctx, &mut name);

    let args = self.args.iter().map(|arg| {
      let into = matches!(arg, Expr::Variable { .. }) && !matches!(arg, Expr::Variable { name: Identifier::Literal(id) } if id == "NULL");

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

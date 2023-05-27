use std::fmt::Debug;

use nom::{branch::permutation, combinator::opt, sequence::tuple, IResult};
use proc_macro2::TokenStream;
use quote::{quote, TokenStreamExt};

use super::*;
use crate::{CodegenContext, LocalContext, MacroToken, ParseContext};

/// A variable declaration.
///
/// ```c
/// #define DECL int var = 77
/// ```
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct Decl {
  pub ty: Type,
  pub name: Expr,
  pub rhs: Expr,
  pub is_static: bool,
}

impl Decl {
  /// Parse a variable declaration.
  pub(crate) fn parse<'i, 't>(
    tokens: &'i [MacroToken<'t>],
    ctx: &ParseContext<'_>,
  ) -> IResult<&'i [MacroToken<'t>], Self> {
    let (tokens, ((static_storage, ty), name, _, rhs)) = tuple((
      permutation((opt(token("static")), |tokens| Type::parse(tokens, ctx))),
      |tokens| Expr::parse_concat_ident(tokens, ctx),
      token("="),
      |tokens| Expr::parse(tokens, ctx),
    ))(tokens)?;

    Ok((tokens, Self { ty, name, rhs, is_static: static_storage.is_some() }))
  }

  pub(crate) fn finish<C>(&mut self, ctx: &mut LocalContext<'_, C>) -> Result<Option<Type>, crate::CodegenError>
  where
    C: CodegenContext,
  {
    self.ty.finish(ctx)?;
    self.name.finish(ctx)?;
    self.rhs.finish(ctx)?;

    Ok(None)
  }

  pub(crate) fn to_tokens<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, C>, tokens: &mut TokenStream) {
    let ty = self.ty.to_token_stream(ctx);
    let name = self.name.to_token_stream(ctx);
    let rhs = self.rhs.to_token_stream(ctx);

    tokens.append_all(if self.is_static {
      quote! { static mut #name: #ty = #rhs }
    } else {
      quote! { let mut #name: #ty = #rhs }
    })
  }

  pub(crate) fn to_token_stream<C: CodegenContext>(&self, ctx: &mut LocalContext<'_, C>) -> TokenStream {
    let mut tokens = TokenStream::new();
    self.to_tokens(ctx, &mut tokens);
    tokens
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  use crate::macro_set::tokens;

  const CTX: ParseContext = ParseContext::var_macro("DECL");

  #[test]
  fn parse() {
    let (_, id) = Decl::parse(tokens!["int", "*", "abc", "=", "123"], &CTX).unwrap();
    assert_eq!(
      id,
      Decl {
        ty: Type::Ptr { ty: Box::new(Type::BuiltIn(BuiltInType::Int)), mutable: true },
        name: var!(abc),
        rhs: Expr::Literal(Lit::Int(LitInt { value: 123, suffix: None })),
        is_static: false,
      }
    );
  }
}

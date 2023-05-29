use std::{borrow::Cow, fmt::Debug};

use nom::{
  branch::alt,
  character::complete::{anychar, char, satisfy},
  combinator::{map, map_opt, recognize},
  multi::{fold_many0, fold_many1},
  sequence::{pair, preceded},
  IResult,
};

use super::{literal::universal_char, tokens::map_token};
use crate::MacroToken;

fn is_identifier_start(c: char) -> bool {
  unicode_ident::is_xid_start(c) || c == '_'
}

pub(crate) fn is_identifier(s: &str) -> bool {
  let mut chars = s.chars();

  if let Some(first_character) = chars.next() {
    return is_identifier_start(first_character) && chars.all(unicode_ident::is_xid_continue)
  }

  false
}

/// A literal identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LitIdent<'t> {
  pub(crate) id: Cow<'t, str>,
}

impl<'t> LitIdent<'t> {
  /// Get the string representation of this identifier.
  pub fn as_str(&self) -> &str {
    self.id.as_ref()
  }

  pub(crate) fn parse_str(input: &'t str) -> IResult<&'t str, Self> {
    map(
      recognize(pair(
        satisfy(is_identifier_start),
        fold_many0(satisfy(unicode_ident::is_xid_continue), || (), |_, _| ()),
      )),
      |s| Self { id: Cow::Borrowed(s) },
    )(input)
  }

  /// Parse an identifier.
  pub(crate) fn parse<'i>(tokens: &'i [MacroToken<'t>]) -> IResult<&'i [MacroToken<'t>], Self> {
    map_token(map_opt(
      |token| {
        fold_many1(
          alt((map_opt(preceded(char('\\'), universal_char), char::from_u32), anychar)),
          String::new,
          |mut acc, c| {
            acc.push(c);
            acc
          },
        )(token)
      },
      |s| {
        if is_identifier(&s) {
          Some(LitIdent { id: Cow::Owned(s) })
        } else {
          None
        }
      },
    ))(tokens)
  }

  pub(crate) fn to_static(&self) -> LitIdent<'static> {
    LitIdent { id: Cow::Owned(self.id.clone().into_owned()) }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  use crate::{lit_id, macro_set::tokens};

  #[test]
  fn parse_literal() {
    let (_, id) = LitIdent::parse(tokens!["asdf"]).unwrap();
    assert_eq!(id, lit_id!(asdf));

    let (_, id) = LitIdent::parse(tokens!["Δx"]).unwrap();
    assert_eq!(id, lit_id!(Δx));

    let (_, id) = LitIdent::parse(tokens!["_123"]).unwrap();
    assert_eq!(id, lit_id!(_123));

    let (_, id) = LitIdent::parse(tokens!["__INT_MAX__"]).unwrap();
    assert_eq!(id, lit_id!(__INT_MAX__));
  }

  #[test]
  fn parse_wrong() {
    let tokens = tokens!["123def"];
    let res = LitIdent::parse(tokens);
    assert!(res.is_err());
  }
}

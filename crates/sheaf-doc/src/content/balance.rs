//! A page's `q`/`Q` balance (§3.4): what content assembly isolates it
//! with, counted from the tokens, with no "unknown".

use super::lex::{LexError, ResCx, Tok};
use super::page::Concat;
use crate::source::Limits;

/// How content's `q`s and `Q`s balance: the net count, and the lowest the
/// running count goes from 0 (never above 0). `q … cm` is `{ net: 1,
/// min_prefix: 0 }`, `Q q` is `{ net: 0, min_prefix: -1 }`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Balance {
    pub net: i32,
    pub min_prefix: i32,
}

impl Balance {
    /// The count after `op`.
    pub(crate) fn take(&mut self, op: &[u8]) {
        match op {
            b"q" => self.net = self.net.saturating_add(1),
            b"Q" => {
                self.net = self.net.saturating_sub(1);
                self.min_prefix = self.min_prefix.min(self.net);
            }
            _ => {}
        }
    }
}

/// How `content`'s `q`s and `Q`s balance, lexed with `res`.
pub fn balance(content: &Concat, res: &ResCx, lim: &Limits) -> Result<Balance, LexError> {
    let mut balance = Balance::default();
    for token in content.lex(res, lim) {
        if let (_, Tok::Op(op)) = token? {
            balance.take(op);
        }
    }
    Ok(balance)
}

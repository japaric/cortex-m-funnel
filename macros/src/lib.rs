extern crate proc_macro;

use core::{fmt::Display, ops::RangeInclusive, str::FromStr};
use proc_macro::TokenStream;
use std::collections::BTreeMap;

use proc_macro2::Span;
use quote::quote;
use syn::{
    braced,
    parse::{self, Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    token, Ident, LitInt, Path, Token,
};

#[proc_macro]
pub fn funnel(input: TokenStream) -> TokenStream {
    match main(parse_macro_input!(input as Input)) {
        Ok(ts) => ts,
        Err(e) => e.to_compile_error().into(),
    }
}

fn main(input: Input) -> parse::Result<TokenStream> {
    const NVIC_PRIO_BITS: &str = "NVIC_PRIO_BITS";
    let nvic_prio_bits = input.nvic_prio_bits.to_string();
    if nvic_prio_bits != NVIC_PRIO_BITS {
        return Err(parse::Error::new(
            input.nvic_prio_bits.span(),
            format!("expected {}, found {}", NVIC_PRIO_BITS, nvic_prio_bits),
        ));
    }

    let (bits, upper) = match input.bits {
        Either::Left(bits) => {
            let bits = lit2ux::<u8>(&bits, Some(1..=8))?;
            (Either::Left(bits), 1 << bits)
        }

        Either::Right(path) => {
            (Either::Right(path), 255)
        }
    };

    let mut map = BTreeMap::new();
    for kv in &input.map {
        let k = lit2ux(&kv.priority, Some(0..=upper))?;
        let v: usize = lit2ux(&kv.size, Some(1..=usize::max_value()))?;

        if map.contains_key(&k) {
            return Err(parse::Error::new(
                kv.priority.span(),
                "priority appears more than once",
            ));
        }

        map.insert(k, v);
    }

    let mut loggers = vec![];
    let mut ls = vec![];
    let mut ifs = vec![];
    for (prio, size) in &map {
        let l = logger_ident(*prio);

        loggers
            .push(quote!(static #l: funnel::Inner<[u8; #size]> = funnel::Inner::new([0; #size]);));
        let (const_, nvic_prio) = match bits {
            Either::Left(bits) => {
                let nvic_prio = ((1 << bits) - prio) << (8 - bits);
                (None, quote!(#nvic_prio))
            }

            Either::Right(ref path) => {
                let px = priority_ident(*prio);
                let const_ = quote!(const #px: u8 = ((1 << #path) - #prio) << (8 - #path););
                (Some(const_), quote!(#px))
            }
        };

        ifs.push(quote!(
            #const_
            if nvic_prio == #nvic_prio {
                return Some(&#l);
            }
        ));

        ls.push(l);
    }

    ls.reverse();
    let n = map.len();
    Ok(quote!(
        const FUNNEL: () = {
            #(#loggers)*
            static D: [&'static funnel::Inner<[u8]>; #n] = [#(&#ls),*];

            #[no_mangle]
            fn __funnel_logger(nvic_prio: u8) -> Option<&'static funnel::Inner<[u8]>> {
                #(#ifs)*

                None
            }

            #[no_mangle]
            fn __funnel_drains() -> &'static [&'static funnel::Inner<[u8]>] {
                &D
            }
        };
    )
    .into())
}

fn logger_ident(prio: u8) -> Ident {
    Ident::new(&format!("L{}", prio), Span::call_site())
}

fn priority_ident(prio: u8) -> Ident {
    Ident::new(&format!("P{}", prio), Span::call_site())
}

fn lit2ux<T>(lit: &LitInt, range: Option<RangeInclusive<T>>) -> parse::Result<T>
where
    T: Copy + Display + FromStr + PartialOrd<T>,
    <T as FromStr>::Err: Display,
{
    if !lit.suffix().is_empty() {
        return Err(parse::Error::new(lit.span(), "literal must be unsuffixed"));
    }

    let n = lit.base10_parse()?;
    if let Some(range) = range {
        if n < *range.start() || n > *range.end() {
            return Err(parse::Error::new(
                lit.span(),
                format!(
                    "literal must be in the range {}..={}",
                    range.start(),
                    range.end()
                ),
            ));
        }
    }

    Ok(n)
}

enum Either<A, B> {
    Left(A),
    Right(B),
}

fn parse_either<A, B>(input: ParseStream) -> parse::Result<Either<A, B>>
where
    A: Parse,
    B: Parse,
{
    if let Ok(a) = input.parse() {
        Ok(Either::Left(a))
    } else {
        Ok(Either::Right(input.parse()?))
    }
}

struct Input {
    nvic_prio_bits: Ident,
    _eq: Token![=],
    bits: Either<LitInt, Path>,
    _comma: Token![,],
    _brace: token::Brace,
    map: Punctuated<KeyValue, Token![,]>,
}

impl Parse for Input {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        let content;
        Ok(Self {
            nvic_prio_bits: input.parse()?,
            _eq: input.parse()?,
            bits: parse_either(input)?,
            _comma: input.parse()?,
            _brace: braced!(content in input),
            map: Punctuated::parse_terminated(&content)?,
        })
    }
}

struct KeyValue {
    priority: LitInt,
    _colon: Token![:],
    size: LitInt,
}

impl Parse for KeyValue {
    fn parse(input: ParseStream) -> parse::Result<Self> {
        Ok(Self {
            priority: input.parse()?,
            _colon: input.parse()?,
            size: input.parse()?,
        })
    }
}

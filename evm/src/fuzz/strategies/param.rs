use ethers::{
    abi::{ParamType, Token, Tokenizable},
    types::{Address, Bytes, I256, U256},
};
use proptest::prelude::*;

use super::state::EvmFuzzState;

/// The max length of arrays we fuzz for is 256.
pub const MAX_ARRAY_LEN: usize = 256;

/// Given a parameter type, returns a strategy for generating values for that type.
///
/// Works with ABI Encoder v2 tuples.
pub fn fuzz_param(param: &ParamType) -> impl Strategy<Value = Token> {
    match param {
        ParamType::Address => {
            // The key to making this work is the `boxed()` call which type erases everything
            // https://altsysrq.github.io/proptest-book/proptest/tutorial/transforming-strategies.html
            any::<[u8; 20]>().prop_map(|x| Address::from_slice(&x).into_token()).boxed()
        }
        ParamType::Bytes => any::<Vec<u8>>().prop_map(|x| Bytes::from(x).into_token()).boxed(),
        // For ints and uints we sample from a U256, then wrap it to the correct size with a
        // modulo operation. Note that this introduces modulo bias, but it can be removed with
        // rejection sampling if it's determined the bias is too severe. Rejection sampling may
        // slow down tests as it resamples bad values, so may want to benchmark the performance
        // hit and weigh that against the current bias before implementing
        ParamType::Int(n) => match n / 8 {
            32 => any::<[u8; 32]>()
                .prop_map(move |x| I256::from_raw(U256::from(&x)).into_token())
                .boxed(),
            y @ 1..=31 => any::<[u8; 32]>()
                .prop_map(move |x| {
                    // Generate a uintN in the correct range, then shift it to the range of intN
                    // by subtracting 2^(N-1)
                    let uint = U256::from(&x) % U256::from(2).pow(U256::from(y * 8));
                    let max_int_plus1 = U256::from(2).pow(U256::from(y * 8 - 1));
                    let num = I256::from_raw(uint.overflowing_sub(max_int_plus1).0);
                    num.into_token()
                })
                .boxed(),
            _ => panic!("unsupported solidity type int{}", n),
        },
        ParamType::Uint(n) => {
            super::UintStrategy::new(*n, vec![]).prop_map(|x| x.into_token()).boxed()
        }
        ParamType::Bool => any::<bool>().prop_map(|x| x.into_token()).boxed(),
        ParamType::String => any::<Vec<u8>>()
            .prop_map(|x| Token::String(unsafe { std::str::from_utf8_unchecked(&x).to_string() }))
            .boxed(),
        ParamType::Array(param) => proptest::collection::vec(fuzz_param(param), 0..MAX_ARRAY_LEN)
            .prop_map(Token::Array)
            .boxed(),
        ParamType::FixedBytes(size) => (0..*size as u64)
            .map(|_| any::<u8>())
            .collect::<Vec<_>>()
            .prop_map(Token::FixedBytes)
            .boxed(),
        ParamType::FixedArray(param, size) => (0..*size as u64)
            .map(|_| fuzz_param(param).prop_map(|param| param.into_token()))
            .collect::<Vec<_>>()
            .prop_map(Token::FixedArray)
            .boxed(),
        ParamType::Tuple(params) => {
            params.iter().map(fuzz_param).collect::<Vec<_>>().prop_map(Token::Tuple).boxed()
        }
    }
}

/// Given a parameter type, returns a strategy for generating values for that type, given some EVM
/// fuzz state.
///
/// Works with ABI Encoder v2 tuples.
pub fn fuzz_param_from_state(param: &ParamType, state: EvmFuzzState) -> BoxedStrategy<Token> {
    // These are to comply with lifetime requirements
    let state_len = state.borrow().len();
    let s = state.clone();

    // Select a value from the state
    let value = any::<prop::sample::Index>()
        .prop_map(move |index| index.index(state_len))
        .prop_map(move |index| *s.borrow().iter().nth(index).unwrap());

    // Convert the value based on the parameter type
    match param {
        ParamType::Address => {
            value.prop_map(move |value| Address::from_slice(&value[12..]).into_token()).boxed()
        }
        ParamType::Bytes => value.prop_map(move |value| Bytes::from(value).into_token()).boxed(),
        ParamType::Int(n) => match n / 8 {
            32 => {
                value.prop_map(move |value| I256::from_raw(U256::from(value)).into_token()).boxed()
            }
            y @ 1..=31 => value
                .prop_map(move |value| {
                    // Generate a uintN in the correct range, then shift it to the range of intN
                    // by subtracting 2^(N-1)
                    let uint = U256::from(value) % U256::from(2usize).pow(U256::from(y * 8));
                    let max_int_plus1 = U256::from(2usize).pow(U256::from(y * 8 - 1));
                    let num = I256::from_raw(uint.overflowing_sub(max_int_plus1).0);
                    num.into_token()
                })
                .boxed(),
            _ => panic!("unsupported solidity type int{}", n),
        },
        ParamType::Uint(n) => match n / 8 {
            32 => value.prop_map(move |value| U256::from(value).into_token()).boxed(),
            y @ 1..=31 => value
                .prop_map(move |value| {
                    (U256::from(value) % (U256::from(2usize).pow(U256::from(y * 8)))).into_token()
                })
                .boxed(),
            _ => panic!("unsupported solidity type uint{}", n),
        },
        ParamType::Bool => value.prop_map(move |value| Token::Bool(value[31] == 1)).boxed(),
        ParamType::String => value
            .prop_map(move |value| {
                Token::String(unsafe { std::str::from_utf8_unchecked(&value[..]).to_string() })
            })
            .boxed(),
        ParamType::Array(param) => {
            proptest::collection::vec(fuzz_param_from_state(param, state), 0..MAX_ARRAY_LEN)
                .prop_map(Token::Array)
                .boxed()
        }
        ParamType::FixedBytes(size) => {
            let size = *size;
            value.prop_map(move |value| Token::FixedBytes(value[32 - size..].to_vec())).boxed()
        }
        ParamType::FixedArray(param, size) => {
            proptest::collection::vec(fuzz_param_from_state(param, state), 0..*size)
                .prop_map(Token::FixedArray)
                .boxed()
        }
        ParamType::Tuple(params) => params
            .iter()
            .map(|p| fuzz_param_from_state(p, state.clone()))
            .collect::<Vec<_>>()
            .prop_map(Token::Tuple)
            .boxed(),
    }
}

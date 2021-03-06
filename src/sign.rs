//! Utility for signing transactions and generating RLP encoded raw transactions.
//! Hopefully we can move this functionailly upstream to the `web3` crate as
//! part of the missing `accounts` namespace.

use crate::secret::PrivateKey;
use ethcontract_common::hash;
use rlp::RlpStream;
use secp256k1::recovery::RecoveryId;
use secp256k1::{Message, Secp256k1};
use web3::types::{Address, Bytes, U256};

/// Raw transaction data to sign
pub struct TransactionData<'a> {
    /// Nonce to use when signing this transaction.
    pub nonce: U256,
    /// Gas price to use when signing this transaction.
    pub gas_price: U256,
    /// Gas provided by the transaction.
    pub gas: U256,
    /// Receiver of the transaction.
    pub to: Option<Address>,
    /// Value of the transaction in wei.
    pub value: U256,
    /// Call data of the transaction, can be empty for simple value transfers.
    pub data: &'a Bytes,
}

impl<'a> TransactionData<'a> {
    /// Sign and return a raw transaction.
    pub fn sign(&self, key: &PrivateKey, chain_id: Option<u64>) -> Bytes {
        let mut rlp = RlpStream::new();
        self.rlp_append_unsigned(&mut rlp, chain_id);

        let hash = hash::keccak256(&rlp.as_raw());
        rlp.clear();

        // NOTE: secp256k1 messages for singing must be exactly 32 bytes long
        //   and not be all `0`s. Because the message being signed here is a 32
        //   byte hash that is computed from non-`0` data (because of RLP
        //   encoding prefixes) the chance of the hash being `0` is
        //   infinitesimally small, so it is OK to unwrap here.
        let message = Message::from_slice(&hash).expect("hash is an invalid secp256k1 message");
        let (recovery_id, sig) = Secp256k1::signing_only()
            .sign_recoverable(&message, &key)
            .serialize_compact();
        self.rlp_append_signed(&mut rlp, recovery_id, sig, chain_id);

        rlp.out().into()
    }

    /// RLP encode an unsigned transaction.
    fn rlp_append_unsigned(&self, s: &mut RlpStream, chain_id: Option<u64>) {
        s.begin_list(if chain_id.is_some() { 9 } else { 6 });
        s.append(&self.nonce);
        s.append(&self.gas_price);
        s.append(&self.gas);
        if let Some(to) = self.to {
            s.append(&to);
        } else {
            s.append(&"");
        }
        s.append(&self.value);
        s.append(&self.data.0);
        if let Some(n) = chain_id {
            s.append(&n);
            s.append(&0u8);
            s.append(&0u8);
        }
    }

    /// RLP encode a transaction with its signature.
    fn rlp_append_signed(
        &self,
        s: &mut RlpStream,
        recovery_id: RecoveryId,
        sig: [u8; 64],
        chain_id: Option<u64>,
    ) {
        let sig_v = add_chain_replay_protection(recovery_id, chain_id);
        let (sig_r, sig_s) = {
            let (mut r, mut s) = ([0u8; 32], [0u8; 32]);
            r.copy_from_slice(&sig[..32]);
            s.copy_from_slice(&sig[32..]);
            (r, s)
        };

        s.begin_list(9);
        s.append(&self.nonce);
        s.append(&self.gas_price);
        s.append(&self.gas);
        if let Some(to) = self.to {
            s.append(&to);
        } else {
            s.append(&"");
        }
        s.append(&self.value);
        s.append(&self.data.0);
        s.append(&sig_v);
        s.append(&U256::from(sig_r));
        s.append(&U256::from(sig_s));
    }
}

/// Encode chain ID based on (EIP-155)[https://github.com/ethereum/EIPs/blob/master/EIPS/eip-155.md)
fn add_chain_replay_protection(recovery_id: RecoveryId, chain_id: Option<u64>) -> u64 {
    (recovery_id.to_i32() as u64)
        + if let Some(n) = chain_id {
            35 + n * 2
        } else {
            27
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign() {
        // retrieved test vector from:
        // https://web3js.readthedocs.io/en/v1.2.0/web3-eth-accounts.html#eth-accounts-signtransaction

        let tx = TransactionData {
            nonce: 0.into(),
            gas: 2_000_000.into(),
            gas_price: 234_567_897_654_321u64.into(),
            to: Some(
                "F0109fC8DF283027b6285cc889F5aA624EaC1F55"
                    .parse()
                    .expect("invalid address"),
            ),
            value: 1_000_000_000.into(),
            data: &Bytes::default(),
        };
        let key = key!("0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318");
        let raw = tx.sign(&key, Some(1));

        let expected = bytes!("0xf86a8086d55698372431831e848094f0109fc8df283027b6285cc889f5aa624eac1f55843b9aca008025a009ebb6ca057a0535d6186462bc0b465b561c94a295bdb0621fc19208ab149a9ca0440ffd775ce91a833ab410777204d5341a6f9fa91216a6f3ee2c051fea6a0428");

        assert_eq!(raw, expected);
    }

    #[test]
    fn test_sign_deploy() {
        // test vector generated with `web3 v1.2.1` with the following code:
        // ```
        // web3.eth.accounts.signTransaction({
        //     nonce: 42,
        //     gas: '2000000',
        //     gasPrice: '6000000000',
        //     value: '0',
        //     data: '0x600080fd', // revert()
        //     chainId: 5777,
        // }, '0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318')
        // ```
        // which produced the following output:
        // ```
        // {
        //     messageHash: '0x0526a7987ac9f046668309e842c25a5388a853f09af138bc614160248d93b8ed',
        //     v: '0x2d45',
        //     r: '0x991b1f1c803676a8a7d9ef09ffd760c0cf94b4e3300670588b98acac01627299',
        //     s: '0x7d06916a45758cdf569c2a8ac5078f58cd955e9b43c7eff8362c2de1c3554ac8',
        //     rawTransaction: '0xf8572a850165a0bc00831e8480808084600080fd822d45a0991b1f1c803676a8a7d9ef09ffd760c0cf94b4e3300670588b98acac01627299a07d06916a45758cdf569c2a8ac5078f58cd955e9b43c7eff8362c2de1c3554ac8',
        // }
        // ```

        let tx = TransactionData {
            nonce: 42.into(),
            gas: 2_000_000.into(),
            gas_price: 6_000_000_000u64.into(),
            to: None,
            value: 0.into(),
            data: &bytes!("0x600080fd"),
        };
        let key = key!("0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318");
        let raw = tx.sign(&key, Some(5777));

        let expected = bytes!("0xf8572a850165a0bc00831e8480808084600080fd822d45a0991b1f1c803676a8a7d9ef09ffd760c0cf94b4e3300670588b98acac01627299a07d06916a45758cdf569c2a8ac5078f58cd955e9b43c7eff8362c2de1c3554ac8");

        assert_eq!(raw, expected);
    }
}

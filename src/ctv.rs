use std::io::{Cursor, Write};

use anyhow::anyhow;
use bitcoin::{
    absolute::LockTime,
    address::{NetworkChecked, NetworkUnchecked},
    consensus::Encodable,
    opcodes::all::{OP_CHECKSIGVERIFY, OP_CSV, OP_ELSE, OP_IF},
    script::{PushBytes, PushBytesBuf},
    transaction::Version,
    Address, Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Witness,
};
use miniscript::bitcoin::opcodes::all::OP_DROP;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ctv {
    pub network: Network,
    pub version: Version,
    pub locktime: LockTime,
    pub sequences: Vec<Sequence>,
    pub outputs: Vec<Output>,
}

impl Ctv {
    pub fn as_tx(&self) -> anyhow::Result<Transaction> {
        let input = self
            .sequences
            .iter()
            .map(|seq| TxIn {
                sequence: *seq,
                ..Default::default()
            })
            .collect();
        let output: anyhow::Result<Vec<TxOut>> = self
            .outputs
            .iter()
            .map(|output| output.as_txout(self.network))
            .collect();
        Ok(Transaction {
            version: self.version,
            lock_time: self.locktime,
            input,
            output: output?,
        })
    }

    pub fn spending_tx(&self, txid: Txid, vout: u32) -> anyhow::Result<Vec<Transaction>> {
        let mut transactions = Vec::new();
        let tx = Transaction {
            version: self.version,
            lock_time: self.locktime,
            input: vec![TxIn {
                previous_output: OutPoint { txid, vout },
                script_sig: Default::default(),
                sequence: *self
                    .sequences
                    .first()
                    .ok_or_else(|| anyhow!("Missing sequence"))?,
                witness: self.witness()?,
            }],
            output: self.txouts()?,
        };
        let current_txid = tx.txid();
        transactions.push(tx);
        if let Some(Output::Tree { tree, amount: _ }) = self.outputs.first() {
            transactions.extend_from_slice(&tree.spending_tx(current_txid, 0)?);
        }
        Ok(transactions)
    }

    pub fn txouts(&self) -> anyhow::Result<Vec<TxOut>> {
        self.outputs
            .iter()
            .map(|output| output.as_txout(self.network))
            .collect()
    }

    pub fn ctv(&self) -> anyhow::Result<Vec<u8>> {
        Ok(util::ctv(&self.as_tx()?, 0))
    }

    fn witness(&self) -> anyhow::Result<Witness> {
        let mut witness = Witness::new();
        let script = segwit::locking_script(&self.ctv()?);
        witness.push(&script);
        Ok(witness)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Output {
    Address {
        address: Address<NetworkUnchecked>,
        amount: Amount,
    },
    Data {
        data: String,
    },
    Tree {
        tree: Box<Ctv>,
        amount: Amount,
    },
}

impl Output {
    pub fn as_txout(&self, network: Network) -> anyhow::Result<TxOut> {
        Ok(match self {
            Output::Address { address, amount } => TxOut {
                value: *amount,
                script_pubkey: address.clone().require_network(network)?.script_pubkey(),
            },
            Output::Data { data } => {
                let mut pb = PushBytesBuf::new();
                pb.extend_from_slice(data.as_bytes())?;
                TxOut {
                    value: Amount::ZERO,
                    script_pubkey: ScriptBuf::new_op_return(&pb),
                }
            }
            Output::Tree { tree, amount } => {
                let tmplhash = tree.ctv()?;
                let locking_script = segwit::locking_script(&tmplhash);
                TxOut {
                    value: *amount,
                    script_pubkey: Address::p2wsh(&locking_script, network).script_pubkey(),
                }
            }
        })
    }
}

mod util {
    use std::io::Cursor;
    use std::io::Write;

    use bitcoin::{consensus::Encodable, Transaction};
    use sha2::{Digest, Sha256};

    pub(super) fn ctv(tx: &Transaction, input: u32) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::<u8>::new());
        tx.version.consensus_encode(&mut buffer).unwrap();
        tx.lock_time.consensus_encode(&mut buffer).unwrap();
        if let Some(scriptsigs) = scriptsigs(tx) {
            buffer.write_all(&scriptsigs).unwrap();
        }
        (tx.input.len() as u32)
            .consensus_encode(&mut buffer)
            .unwrap();
        buffer.write_all(&sequences(tx)).unwrap();
        (tx.output.len() as u32)
            .consensus_encode(&mut buffer)
            .unwrap();
        buffer.write_all(&outputs(tx)).unwrap();
        input.consensus_encode(&mut buffer).unwrap();
        let buffer = buffer.into_inner();
        sha256(buffer)
    }

    fn scriptsigs(tx: &Transaction) -> Option<Vec<u8>> {
        // If there are no scripts sigs, do nothing
        if tx.input.iter().all(|txin| txin.script_sig.is_empty()) {
            return None;
        }

        let scripts_sigs = tx
            .input
            .iter()
            .fold(Cursor::new(Vec::new()), |mut cursor, txin| {
                txin.script_sig.consensus_encode(&mut cursor).unwrap();
                cursor
            })
            .into_inner();
        Some(sha256(scripts_sigs))
    }

    fn sequences(tx: &Transaction) -> Vec<u8> {
        let sequences = tx
            .input
            .iter()
            .fold(Cursor::new(Vec::new()), |mut cursor, txin| {
                txin.sequence.consensus_encode(&mut cursor).unwrap();
                cursor
            })
            .into_inner();
        sha256(sequences)
    }

    fn outputs(tx: &Transaction) -> Vec<u8> {
        let outputs = tx
            .output
            .iter()
            .fold(Cursor::new(Vec::new()), |mut cursor, txout| {
                txout.consensus_encode(&mut cursor).unwrap();
                cursor
            })
            .into_inner();
        sha256(outputs)
    }

    pub fn sha256(data: Vec<u8>) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().to_vec()
    }
}

pub fn colorize(script: &str) -> String {
    let opcode = Regex::new(r"(OP_\w+)").unwrap();
    let hex = Regex::new(r"([0-9a-z]{64})").unwrap();
    let color = opcode.replace_all(script, r#"<span style="color: red">$1</span>"#);
    let color = hex.replace_all(&color, r#"<span style="color: green">$1</span>"#);

    color.replace("OP_NOP4", "OP_CTV")
}

pub mod segwit {
    use bitcoin::{
        absolute::LockTime,
        address::NetworkUnchecked,
        opcodes::all::{OP_0NOTEQUAL, OP_CSV, OP_DROP, OP_ELSE, OP_ENDIF, OP_IF, OP_NOP4},
        script::PushBytesBuf,
        transaction::Version,
        Address, Amount, Network, Script, ScriptBuf, Sequence,
    };

    use super::{Ctv, Output};

    pub fn locking_address(script: &Script, network: Network) -> Address {
        Address::p2wsh(script, network)
    }

    pub fn locking_script(tmplhash: &[u8]) -> ScriptBuf {
        let bytes = <&[u8; 32]>::try_from(tmplhash).unwrap();
        bitcoin::script::Builder::new()
            .push_slice(bytes)
            .push_opcode(OP_NOP4)
            .into_script()
    }

    pub fn vault_locking_script(
        delay: u16,
        cold: Address<NetworkUnchecked>,
        hot: Address<NetworkUnchecked>,
        network: Network,
        amount: Amount,
    ) -> anyhow::Result<ScriptBuf> {
        let cold_ctv = Ctv {
            network,
            version: Version::ONE,
            locktime: LockTime::ZERO,
            sequences: vec![Sequence::ZERO],
            outputs: vec![Output::Address {
                address: cold,
                amount: amount - Amount::from_sat(600),
            }],
        };
        let cold_hash = PushBytesBuf::try_from(cold_ctv.ctv()?)?;
        let mut hot_ctv = cold_ctv.clone();
        hot_ctv.outputs[0] = Output::Address {
            address: hot,
            amount: amount - Amount::from_sat(600),
        };
        let hot_hash = PushBytesBuf::try_from(hot_ctv.ctv()?)?;
        Ok(bitcoin::script::Builder::new()
            .push_opcode(OP_IF)
            .push_sequence(Sequence::from_height(delay))
            .push_opcode(OP_CSV)
            .push_opcode(OP_DROP)
            .push_slice(hot_hash)
            .push_opcode(OP_NOP4)
            .push_opcode(OP_ELSE)
            .push_slice(cold_hash)
            .push_opcode(OP_NOP4)
            .push_opcode(OP_ENDIF)
            .into_script())
    }
}

#[cfg(test)]
mod tests {
    use crate::ctv::util::ctv;
    use serde_json::Value;

    use super::*;

    #[test]
    fn test_ctv() {
        let test_data = include_str!("../tests/ctvhash.json");
        let test_data: Vec<Value> = serde_json::from_str(test_data).unwrap();
        for td in test_data {
            if td.is_string() {
                continue;
            }
            let td = td.as_object().unwrap();
            let hex_tx = td["hex_tx"].as_str().unwrap();
            let tx: Transaction =
                bitcoin::consensus::deserialize(&hex::decode(hex_tx).unwrap()).unwrap();
            let spend_index = td["spend_index"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i.as_i64().unwrap())
                .collect::<Vec<i64>>();
            let result: Vec<String> = td["result"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();

            for (idx, si) in spend_index.into_iter().enumerate() {
                let hash = hex::encode(ctv(&tx, si as u32));
                assert_eq!(hash, result[idx]);
            }
        }
    }
}

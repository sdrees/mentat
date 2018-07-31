// Copyright 2016 Mozilla
//
// Licensed under the Apache License, Version 2.0 (the "License"); you may not use
// this file except in compliance with the License. You may obtain a copy of the
// License at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software distributed
// under the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR
// CONDITIONS OF ANY KIND, either express or implied. See the License for the
// specific language governing permissions and limitations under the License.

extern crate uuid;
extern crate edn;
extern crate mentat;
extern crate mentat_core;

#[macro_use] extern crate log;
#[macro_use] extern crate mentat_db;

#[cfg(feature = "syncable")]
extern crate mentat_tolstoy;

#[cfg(feature = "syncable")]
mod tests {
    use std::collections::HashMap;
    use std::collections::BTreeMap;

    use std::collections::hash_map::Entry;

    use std::borrow::Borrow;

    use uuid::Uuid;

    use edn;

    use mentat::conn::Conn;

    use mentat_db::debug::{
        TestConn,
    };

    use mentat::new_connection;
    use mentat_tolstoy::{
        Tx,
        TxPart,
        GlobalTransactionLog,
        SyncReport,
        Syncer,
    };

    use mentat_tolstoy::debug::{
        parts_to_datoms,
    };

    use mentat_tolstoy::tx_processor::{
        Processor,
        TxReceiver,
    };
    use mentat_tolstoy::errors::Result;
    use mentat_core::{
        Entid,
        TypedValue,
        ValueType,
    };

    struct TxCountingReceiver {
        pub tx_count: usize,
        pub is_done: bool,
    }

    impl TxCountingReceiver {
        fn new() -> TxCountingReceiver {
            TxCountingReceiver {
                tx_count: 0,
                is_done: false,
            }
        }
    }

    impl TxReceiver for TxCountingReceiver {
        fn tx<T>(&mut self, _tx_id: Entid, _d: &mut T) -> Result<()>
            where T: Iterator<Item=TxPart> {
            self.tx_count = self.tx_count + 1;
            Ok(())
        }

        fn done(&mut self) -> Result<()> {
            self.is_done = true;
            Ok(())
        }
    }

    #[derive(Debug)]
    struct TestingReceiver {
        pub txes: BTreeMap<Entid, Vec<TxPart>>,
        pub is_done: bool,
    }

    impl TestingReceiver {
        fn new() -> TestingReceiver {
            TestingReceiver {
                txes: BTreeMap::new(),
                is_done: false,
            }
        }
    }

    impl TxReceiver for TestingReceiver {
        fn tx<T>(&mut self, tx_id: Entid, d: &mut T) -> Result<()>
            where T: Iterator<Item=TxPart> {
            let datoms = self.txes.entry(tx_id).or_insert(vec![]);
            datoms.extend(d);
            Ok(())
        }

        fn done(&mut self) -> Result<()> {
            self.is_done = true;
            Ok(())
        }
    }

    fn assert_tx_datoms_count(receiver: &TestingReceiver, tx_num: usize, expected_datoms: usize) {
        let tx = receiver.txes.keys().nth(tx_num).expect("first tx");
        let datoms = receiver.txes.get(tx).expect("datoms");
        assert_eq!(expected_datoms, datoms.len());
    }

    #[test]
    fn test_reader() {
        let mut c = new_connection("").expect("Couldn't open conn.");
        let mut conn = Conn::connect(&mut c).expect("Couldn't open DB.");
        {
            let db_tx = c.transaction().expect("db tx");
            // Ensure that we see a bootstrap transaction.
            let mut receiver = TxCountingReceiver::new();
            assert_eq!(false, receiver.is_done);
            Processor::process(&db_tx, None, &mut receiver).expect("processor");
            assert_eq!(true, receiver.is_done);
            assert_eq!(1, receiver.tx_count);
        }

        let ids = conn.transact(&mut c, r#"[
            [:db/add "s" :db/ident :foo/numba]
            [:db/add "s" :db/valueType :db.type/long]
            [:db/add "s" :db/cardinality :db.cardinality/one]
        ]"#).expect("successful transaction").tempids;
        let numba_entity_id = ids.get("s").unwrap();

        let ids = conn.transact(&mut c, r#"[
            [:db/add "b" :foo/numba 123]
        ]"#).expect("successful transaction").tempids;
        let _asserted_e = ids.get("b").unwrap();

        let first_tx;
        {
            let db_tx = c.transaction().expect("db tx");
            // Expect to see one more transaction of four parts (one for tx datom itself).
            let mut receiver = TestingReceiver::new();
            Processor::process(&db_tx, None, &mut receiver).expect("processor");

            println!("{:#?}", receiver);

            // Three transactions: bootstrap, vocab, assertion.
            assert_eq!(3, receiver.txes.keys().count());
            assert_tx_datoms_count(&receiver, 2, 2);

            first_tx = Some(*receiver.txes.keys().nth(1).expect("first non-bootstrap tx"));
        }

        let ids = conn.transact(&mut c, r#"[
            [:db/add "b" :foo/numba 123]
        ]"#).expect("successful transaction").tempids;
        let asserted_e = ids.get("b").unwrap();

        {
            let db_tx = c.transaction().expect("db tx");

            // Expect to see a single two part transaction
            let mut receiver = TestingReceiver::new();

            // Note that we're asking for the first transacted tx to be skipped by the processor.
            Processor::process(&db_tx, first_tx, &mut receiver).expect("processor");

            // Vocab, assertion.
            assert_eq!(2, receiver.txes.keys().count());
            // Assertion datoms.
            assert_tx_datoms_count(&receiver, 1, 2);

            // Inspect the assertion.
            let tx_id = receiver.txes.keys().nth(1).expect("tx");
            let datoms = receiver.txes.get(tx_id).expect("datoms");
            let part = datoms.iter().find(|&part| &part.e == asserted_e).expect("to find asserted datom");

            assert_eq!(numba_entity_id, &part.a);
            assert!(part.v.matches_type(ValueType::Long));
            assert_eq!(TypedValue::Long(123), part.v);
            assert_eq!(true, part.added);
        }
    }

    struct TestRemoteClient {
        pub head: Uuid,
        pub chunks: HashMap<Uuid, TxPart>,
        pub transactions: HashMap<Uuid, Vec<TxPart>>,
        // Keep transactions in order:
        pub tx_rowid: HashMap<Uuid, usize>,
        pub rowid_tx: Vec<Uuid>,
    }

    impl TestRemoteClient {
        fn new() -> TestRemoteClient {
            TestRemoteClient {
                head: Uuid::nil(),
                chunks: HashMap::default(),
                transactions: HashMap::default(),
                tx_rowid: HashMap::default(),
                rowid_tx: vec![],
            }
        }
    }

    impl GlobalTransactionLog for TestRemoteClient {
        fn head(&self) -> Result<Uuid> {
            Ok(self.head)
        }

        fn transactions_after(&self, tx: &Uuid) -> Result<Vec<Tx>> {
            let rowid = self.tx_rowid[tx];
            let mut txs = vec![];
            for tx_uuid in &self.rowid_tx[rowid + 1..] {
                txs.push(Tx {
                    tx: tx_uuid.clone(),
                    parts: self.transactions.get(tx_uuid).unwrap().clone(),
                });
            }
            Ok(txs)
        }

        fn set_head(&mut self, tx: &Uuid) -> Result<()> {
            self.head = tx.clone();
            Ok(())
        }

        fn put_chunk(&mut self, tx: &Uuid, payload: &TxPart) -> Result<()> {
            match self.chunks.entry(tx.clone()) {
                Entry::Occupied(_) => panic!("trying to overwrite chunk"),
                Entry::Vacant(entry) => {
                    entry.insert(payload.clone());
                    ()
                },
            }
            Ok(())
        }

        fn put_transaction(&mut self, tx: &Uuid, _parent_tx: &Uuid, chunk_txs: &Vec<Uuid>) -> Result<()> {
            let mut parts = vec![];
            for chunk_tx in chunk_txs {
                parts.push(self.chunks.get(chunk_tx).unwrap().clone());
            }
            self.transactions.insert(tx.clone(), parts);
            self.rowid_tx.push(tx.clone());
            self.tx_rowid.insert(tx.clone(), self.rowid_tx.len());
            Ok(())
        }
    }

    // TODO these tests have a problem:
    // they want to use assert_transact!, but that macro operates over a TestConn,
    // and we only have a real Conn here.
    // Adapt the macro, and tests should work!

    // #[test]
    // fn test_sync() {
    //     let mut sqlite = new_connection("").unwrap();
    //     let mut conn = Conn::connect(&mut sqlite).unwrap();

    //     let mut local = conn.begin_transaction(&mut sqlite).expect("begun successfully");

    //     let mut remote_client = TestRemoteClient::new();

    //     // Populate empty server, populate it with a bootstrap transaction.
    //     assert_eq!(SyncReport::ServerFastForward, Syncer::sync(&mut local, &mut remote_client).expect("sync report"));

    //     // A single (bootstrap) transaction...
    //     assert_eq!(1, remote_client.transactions.iter().count());
    //     // ... consisting of 95 parts.
    //     assert_eq!(95, remote_client.transactions.values().last().unwrap().len());
    //     // Chunk count matches transaction parts count.
    //     assert_eq!(95, remote_client.chunks.iter().count());

    //     // Nothing changed locally, nor remotely.
    //     assert_eq!(SyncReport::NoChanges, Syncer::sync(&mut local, &mut remote_client).expect("sync report"));

    //     assert_transact!(conn, "[
    //         {:db/ident :person/name
    //           :db/valueType :db.type/string
    //           :db/cardinality :db.cardinality/many}]");

    //     assert_transact!(conn, "[{:person/name \"Ivan\"}]");

    //     // Fast-forward server.
    //     assert_eq!(SyncReport::ServerFastForward, Syncer::sync(&mut local, &mut remote_client).expect("sync report"));

    //     // Uploaded two transactions...
    //     assert_eq!(3, remote_client.transactions.iter().count());
    //     // ... first one of 4 parts (three explicit datoms + txInstant assertion),
    //     let first_tx_parts = remote_client.transactions.get(&remote_client.rowid_tx[1]).unwrap();
    //     assert_eq!(4, first_tx_parts.len());
    //     // ... second of 2 parts.
    //     let second_tx_parts = remote_client.transactions.get(&remote_client.rowid_tx[2]).unwrap();
    //     assert_eq!(2, second_tx_parts.len());

    //     // TODO improve this matching.

    //     // Compare what was uploaded with our local state.
    //     assert_matches!(parts_to_datoms(&local.schema, first_tx_parts.to_vec()), "[
    //         [:person/name :db/ident :person/name 268435457 true]
    //         [268435457 :db/txInstant ?ms 268435457 true]
    //         [:person/name :db/valueType 27 268435457 true]
    //         [:person/name :db/cardinality 34 268435457 true]]");

    //     assert_matches!(parts_to_datoms(&local.schema, second_tx_parts.to_vec()), "[
    //         [65537 :person/name \"Ivan\" 268435458 true]
    //         [268435458 :db/txInstant ?ms 268435458 true]]");
    // }
}

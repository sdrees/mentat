// Copyright 2018 Mozilla
//
// Licensed under the Apache License, Version 2.0 (the "License"); you may not use
// this file except in compliance with the License. You may obtain a copy of the
// License at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software distributed
// under the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR
// CONDITIONS OF ANY KIND, either express or implied. See the License for the
// specific language governing permissions and limitations under the License.

use std; // To refer to std::result::Result.

use rusqlite;

use std::error::Error;
use std::collections::BTreeSet;

use edn;
use mentat_db;
use mentat_query_algebrizer;
use mentat_query_projector;
use mentat_query_pull;
use mentat_sql;

pub type Result<T> = std::result::Result<T, TransactionError>;

#[macro_export]
macro_rules! bail {
    ($e:expr) => (
        return Err($e.into());
    )
}

#[derive(Debug, Fail)]
pub enum TransactionError {
    #[fail(display = "variables {:?} unbound at query execution time", _0)]
    UnboundVariables(BTreeSet<String>),

    #[fail(display = "unknown attribute: '{}'", _0)]
    UnknownAttribute(String),

    #[fail(display = "Lost the transact() race!")]
    UnexpectedLostTransactRace,

    #[fail(display = "{}", _0)]
    AlgebrizerError(#[cause] mentat_query_algebrizer::AlgebrizerError),

    #[fail(display = "{}", _0)]
    ProjectorError(#[cause] mentat_query_projector::ProjectorError),

    #[fail(display = "{}", _0)]
    EdnParseError(#[cause] edn::ParseError),

    #[fail(display = "{}", _0)]
    PullError(#[cause] mentat_query_pull::PullError),

    #[fail(display = "{}", _0)]
    SQLError(#[cause] mentat_sql::SQLError),

    #[fail(display = "{}", _0)]
    DbError(#[cause] mentat_db::DbError),

    // It would be better to capture the underlying `rusqlite::Error`, but that type doesn't
    // implement many useful traits, including `Clone`, `Eq`, and `PartialEq`.
    #[fail(display = "SQL error: {}, cause: {}", _0, _1)]
    RusqliteError(String, String),

    #[fail(display = "{}", _0)]
    IoError(#[cause] std::io::Error),
}

impl From<rusqlite::Error> for TransactionError {
    fn from(error: rusqlite::Error) -> TransactionError {
        let cause = match error.cause() {
            Some(e) => e.to_string(),
            None => "".to_string()
        };
        TransactionError::RusqliteError(error.to_string(), cause)
    }
}

impl From<std::io::Error> for TransactionError {
    fn from(error: std::io::Error) -> TransactionError {
        TransactionError::IoError(error)
    }
}

impl From<edn::ParseError> for TransactionError {
    fn from(error: edn::ParseError) -> TransactionError {
        TransactionError::EdnParseError(error)
    }
}

impl From<mentat_db::DbError> for TransactionError {
    fn from(error: mentat_db::DbError) -> TransactionError {
        TransactionError::DbError(error)
    }
}

impl From<mentat_query_algebrizer::AlgebrizerError> for TransactionError {
    fn from(error: mentat_query_algebrizer::AlgebrizerError) -> TransactionError {
        TransactionError::AlgebrizerError(error)
    }
}

impl From<mentat_query_projector::ProjectorError> for TransactionError {
    fn from(error: mentat_query_projector::ProjectorError) -> TransactionError {
        TransactionError::ProjectorError(error)
    }
}

impl From<mentat_query_pull::PullError> for TransactionError {
    fn from(error: mentat_query_pull::PullError) -> TransactionError {
        TransactionError::PullError(error)
    }
}

impl From<mentat_sql::SQLError> for TransactionError {
    fn from(error: mentat_sql::SQLError) -> TransactionError {
        TransactionError::SQLError(error)
    }
}

/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_error::BuckErrorContext;
use buck2_hash::IntentionallyStdHashMap;
use itertools::Itertools;
use parking_lot::Mutex;
use rusqlite::Connection;

/// A generic sqlite table for storing string key-value pairs.
pub struct KeyValueSqliteTable {
    table_name: String,
    connection: Arc<Mutex<Connection>>,
}

impl KeyValueSqliteTable {
    pub fn new(table_name: String, connection: Arc<Mutex<Connection>>) -> Self {
        Self {
            table_name,
            connection,
        }
    }

    pub fn create_table(&self) -> buck2_error::Result<()> {
        let sql = format!(
            "CREATE TABLE {} (
                key     TEXT PRIMARY KEY NOT NULL,
                value   TEXT NOT NULL
            )",
            self.table_name
        );
        tracing::trace!(sql = %sql, "creating table");
        self.connection
            .lock()
            .execute(&sql, [])
            .with_buck_error_context(|| format!("creating sqlite table {}", self.table_name))?;
        Ok(())
    }

    pub fn insert_all(
        &self,
        map: IntentionallyStdHashMap<String, String>,
    ) -> buck2_error::Result<()> {
        let sql = format!(
            "INSERT OR REPLACE INTO {} (key, value) VALUES {}",
            self.table_name,
            // According to rusqlite docs this is the recommended way to generate the right
            // number of query placeholders for multi-row insertions.
            map.iter().map(|_| "(?, ?)").join(", ")
        );
        tracing::trace!(sql = %sql, map = ?map, "inserting into table");

        self.connection
            .lock()
            .execute(
                &sql,
                rusqlite::params_from_iter(map.into_iter().flat_map(<[_; 2]>::from)),
            )
            .with_buck_error_context(|| {
                format!("inserting into sqlite table {}", self.table_name)
            })?;
        Ok(())
    }

    pub fn read_all(&self) -> buck2_error::Result<IntentionallyStdHashMap<String, String>> {
        let sql = format!("SELECT key, value FROM {}", self.table_name);
        tracing::trace!(sql = %sql, "read all from table");
        let connection = self.connection.lock();
        let mut stmt = connection.prepare(&sql)?;
        let map = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<IntentionallyStdHashMap<String, String>, _>>()
            .with_buck_error_context(|| format!("reading from sqlite table {}", self.table_name))?;
        Ok(map)
    }

    pub fn get(&self, key: &str) -> buck2_error::Result<Option<String>> {
        let sql = format!("SELECT value FROM {} WHERE key = ?", self.table_name);
        tracing::trace!(sql = %sql, key = %key, "read from table");
        let connection = self.connection.lock();
        let mut stmt = connection.prepare(&sql)?;
        let row = stmt
            .query_map([key], |row| row.get(0))?
            .next()
            .transpose()
            .with_buck_error_context(|| {
                format!("reading `{}` from sqlite table {}", key, self.table_name)
            })?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {

    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;

    use super::*;

    #[test]
    fn test_key_value_sqlite_table() {
        let fs = ProjectRootTemp::new().unwrap();
        let connection = Connection::open(
            fs.path()
                .resolve(ProjectRelativePath::unchecked_new("test.db")),
        )
        .unwrap();
        let table =
            KeyValueSqliteTable::new("metadata".to_owned(), Arc::new(Mutex::new(connection)));

        table.create_table().unwrap();

        let expected = IntentionallyStdHashMap::from([
            ("foo".to_owned(), "foo".to_owned()),
            ("bar".to_owned(), "bar".to_owned()),
        ]);
        table.insert_all(expected.clone()).unwrap();

        let actual = table.read_all().unwrap();
        assert_eq!(expected, actual);

        assert_eq!(table.get("foo").unwrap().as_deref(), Some("foo"));
        assert_eq!(table.get("baz").unwrap(), None);
    }
}

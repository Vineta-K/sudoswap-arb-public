use ethers::prelude::*;
use eyre::eyre;
use rusqlite::types::{FromSql, ToSql};
use rusqlite::{Connection, Row, params, Statement};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::events::Event;
pub struct SqlAddress(Address);

impl FromSql for SqlAddress {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let value = value
            .as_str()?
            .parse::<Address>()
            .expect("Error converting from SQL string to address");
        Ok(SqlAddress(value))
    }
}

impl ToSql for SqlAddress {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        let output = format!("{:?}", self.to_address());
        Ok(output.into())
    }
}

impl SqlAddress {
    pub fn to_address(&self) -> Address {
        let SqlAddress(address) = *self;
        address
    }
}

pub async fn create_db_tables_if_not_exists(db_name: &PathBuf) -> eyre::Result<()> {
    let conn = Connection::open(db_name)?;
    conn.execute(
        "create table if not exists state(
            i integer primary key,
            block_number integer,
            data text
        )",
        [],
    )?;
    conn.execute(
        "create table if not exists nft_list(
            i integer primary key,
            banned_nfts text unique,
            approved_sudo text,
            approved_sudo_pool text unique,
            approved_nftx text unique
        )",
        [],
    )?;
    conn.execute(
        "create table if not exists events_json(
            i integer primary key,
            data text
        )",
        [],
    )?;
    Ok(())
}

pub async fn set_init_db_values(db_name: &PathBuf, deployed_block: u64) -> eyre::Result<U64> {
    let conn = Connection::open(db_name)?;
    conn.execute(
        "insert into state (block_number) values (?1)",
        [deployed_block],
    )?;
    Ok(U64::from(deployed_block))
}

pub async fn add_approved_nft(
    db_name: &PathBuf,
    nft_address: Address,
    pool_address: Address,
    market: &str,
) -> eyre::Result<()> {
    let conn = Connection::open(db_name)?;
    match market {
        "sudo" => {
            conn.execute(
                "insert into nft_list (approved_sudo, approved_sudo_pool) values (?1,?2)",
                params![SqlAddress(nft_address), SqlAddress(pool_address)],
            )?;
        }
        "nftx" => {
            conn.execute(
                "replace into nft_list (approved_nftx) values (?1)",
                [SqlAddress(nft_address)],
            )?;
        }
        _ => todo!(),
    }
    Ok(())
}

pub async fn add_banned_nft(db_name: &PathBuf, nft_address: Address) -> eyre::Result<()> {
    let conn = Connection::open(db_name)?;
    conn.execute(
        "replace into nft_list (banned_nfts) values (?1)",
        [SqlAddress(nft_address)],
    )?;
    Ok(())
}

pub async fn read_approved_nft_list(
    db_name: &PathBuf,
) -> eyre::Result<(HashSet<(Address, Address)>, HashSet<Address>)> {
    let conn = Connection::open(db_name)?;
    let mut stmt = conn.prepare("select * from nft_list")?;
    let approved = stmt.query_map(
        [],
        |row: &Row| -> Result<
            (
                Result<SqlAddress, rusqlite::Error>,
                Result<SqlAddress, rusqlite::Error>,
            ),
            rusqlite::Error,
        > { Ok((row.get(2), row.get(3))) },
    )?; //WTF is this
    let approved_sudo = approved
        .map(|row| match row.unwrap() {
            (Ok(nft), Ok(pool)) => (nft.to_address(), pool.to_address()),
            _ => (Address::zero(), Address::zero()),
        })
        .collect::<HashSet<(_, _)>>();
    let approved = stmt.query_map([], |row| -> Result<SqlAddress, rusqlite::Error> {
        row.get(4)
    })?; //WTF is this
    let approved_nftx = approved
        .map(|row| match row {
            Ok(row) => row.to_address(),
            Err(_) => Address::zero(),
        })
        .collect::<HashSet<_>>();
    Ok((approved_sudo, approved_nftx))
}

pub async fn read_banned_nft_list(db_name: &PathBuf) -> eyre::Result<Vec<Address>> {
    let conn = Connection::open(db_name)?;
    let mut stmt = conn.prepare("select * from banlist")?;
    let banned_list = stmt.query_map([], |row| -> Result<SqlAddress, rusqlite::Error> {
        row.get(1)
    })?; //WTF is this
    let banned_list = banned_list
        .map(|row| match row {
            Ok(row) => row.to_address(),
            Err(_) => Address::zero(),
        })
        .collect::<Vec<_>>();
    Ok(banned_list)
}

pub async fn read_last_db_block(db_name: &PathBuf) -> eyre::Result<U64> {
    let conn = Connection::open(db_name)?;
    let mut stmt = conn.prepare("select block_number from state")?;
    let mut blocks = Vec::<u64>::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        blocks.push(row.get(0)?);
    }
    let blocks = blocks.iter().map(|x| U64::from(*x));
    blocks
        .last()
        .ok_or_else(|| eyre!("Error finding last value"))
    //What a hack lol
}

//Some modified code from internet that makes it 100x faster for large inserts by preparing before commiting txs
pub struct DbContext<'a> {
    pub conn: &'a Connection,
    pub dump_event_json_stmt: Option<Statement<'a>>,
}
impl<'a> DbContext<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        DbContext {
            conn,
            dump_event_json_stmt: None,
        }
    }
    pub fn dump_event_json(&mut self, event: &Event) -> Result<i64, rusqlite::Error> {
        if self.dump_event_json_stmt.is_none() {
            let stmt = self
                .conn
                .prepare("INSERT INTO events_json (data) VALUES (:data)")?;
            self.dump_event_json_stmt = Some(stmt);
        };
        self.dump_event_json_stmt
            .as_mut()
            .unwrap()
            .execute(&[(":data", &event)])?;
        Ok(self.conn.last_insert_rowid())
    }
}
pub async fn dump_events_json(db_name: &PathBuf, events: &Vec<Event>) -> eyre::Result<()> {
    let conn = Connection::open(db_name)?;
    let mut ctxt = DbContext::new(&conn);
    ctxt.conn.execute_batch("BEGIN TRANSACTION;")?;
    for event in events {
        ctxt.dump_event_json(event)?;
    }
    ctxt.conn.execute_batch("COMMIT TRANSACTION;")?;
    Ok(())
}
pub async fn read_events_json(db_name: &PathBuf) -> eyre::Result<Vec<Event>> {
    let conn = Connection::open(db_name)?;
    let mut stmt = conn.prepare("select * from events_json")?;
    let events = stmt.query_map([], |row| row.get(1))?;
    let mut events_vec = Vec::new();
    for event in events {
        events_vec.push(event?);
    }
    Ok(events_vec)
}
pub async fn save_block_number(db_name: &PathBuf, last_block: U64) -> eyre::Result<()> {
    let conn = Connection::open(db_name)?;
    conn.execute(
        "insert or replace into state (block_number) values (?1)
        ",
        rusqlite::params![last_block.as_u64()],
    )?;
    Ok(())
}
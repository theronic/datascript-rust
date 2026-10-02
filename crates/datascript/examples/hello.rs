//! DataScript from Rust: a database, a transaction, a query and a pull.
//!
//!     cargo run -p datascript --example hello

use datascript::{db_with, edn, print::pr_str, pull, q, Db, Result, Value};

fn main() -> Result<()> {
    let schema = edn::read_string("{:aka {:db/cardinality :db.cardinality/many}}")?;
    let db = db_with(&Db::empty(schema)?, &edn::read_string(r#"[{:db/id -1 :name "Ivan" :aka ["Devil" "Tupen"]}]"#)?)?;

    let query = edn::read_string(r#"[:find ?n :where [?e :aka "Tupen"] [?e :name ?n]]"#)?;
    let found = q(&query, &[Value::Db(db.clone())])?;
    assert_eq!(pr_str(&found), r#"#{["Ivan"]}"#);
    println!("{}", pr_str(&found));

    // in the order ClojureScript DataScript answers in
    let ivan = pull(&db, &edn::read_string("[:name :aka]")?, &Value::from(1), None)?;
    assert_eq!(pr_str(&ivan), r#"{:aka ["Devil" "Tupen"], :name "Ivan"}"#);
    println!("{}", pr_str(&ivan));
    Ok(())
}

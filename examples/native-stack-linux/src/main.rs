fn main() {
    println!("openssl={}", openssl::version::version());

    let connection = rusqlite::Connection::open_in_memory().expect("open sqlite");
    connection
        .execute(
            "CREATE TABLE artifacts (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            [],
        )
        .expect("create table");
    connection
        .execute("INSERT INTO artifacts (name) VALUES (?1)", ["rcc-musl"])
        .expect("insert row");
    let name: String = connection
        .query_row("SELECT name FROM artifacts", [], |row| row.get(0))
        .expect("select row");
    println!("sqlite={name}");
}

fn main() {
    let _ = sqlx_turso::query!("SELECT missing_column FROM records");
}

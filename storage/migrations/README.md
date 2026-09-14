# Migrations

There are no business migrations in the template. SQLx's `_sqlx_migrations`
metadata is expected even for this empty set. Add the first business migration
with the first actual use case; never modify a file already applied to a database.
`build.rs` tracks this directory, and `.gitattributes` keeps SQL bytes LF-stable.

# Feedgen
An RSS feed generator for external websites.
Fetches pages periodically and extracts entries based on configuration rules.

Supported extractors:

- XPath (extractor kind `xpath`).

## Building
Use `cargo`:

```sh
cargo build --release
```

You'll find the compiled binary at `target/release/feedgen`.

## Configuration
See [`feedgen.example.toml`](feedgen.example.toml) for config file
documentation.

## Usage
For a list of command-line options, run:

```sh
feedgen --help
```

Once Feedgen is running, a web interface will be served at the provided address
with a list of all configured feeds.
Point your RSS reader to the listed RSS feed links.

To force a feed update without waiting for the next scheduled update, send a
POST request to `/feeds/:name/update`.

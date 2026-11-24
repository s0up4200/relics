# Follow Silo

Relics builds against one commit of the Silo server, the pin. The pin is in `tests/fixtures/silo/SERVER_COMMIT`. The wire types in `src/api/wire.rs` and the fixtures in `tests/fixtures/silo/` come from that commit. `api/operations.txt` lists the calls that Relics makes.

## The drift issue

The Silo drift workflow runs each Monday, and also when a change to the pin, to `api/operations.txt`, or to the script reaches `main`. It compares the API contract of Silo `main` with the contract at the pin, and writes the report into one open issue with the label `silo-drift`. When nothing is left to report, the workflow closes the issue.

The report has these parts:

- **Changes to check**: changes that can break a request that Relics sends, or a response that Relics reads. For example, a removed field, a new required field in a request, a field that a response can now leave out, or a changed limit.
- **New fields and values**: Relics can ignore these. Code that matches on known values can still meet a new value.
- **New calls outside the admin API**: calls that Relics does not use yet.
- A count of the calls and schemas that changed only in their descriptions, or in rules for values that Silo sends.

The report covers only the contract in `contracts/api/v2/openapi.json`. It does not see a change to the events of Live updates (ADR 0006), or a change in behavior that the contract does not describe.

To see the report on your computer, run `scripts/upstream-drift.py`. To compare with another Silo branch or commit, give it as the argument.

## Update the pin

1. Get the Silo commit that the report names:

   ```sh
   git clone https://github.com/Silo-Server/silo-server ~/github/oss/silo-server
   git -C ~/github/oss/silo-server checkout <commit>
   ```

2. Fix Relics for each change to check.
3. To use a new call, add its operationId to `api/operations.txt`.
4. Run `scripts/refresh-fixtures.py`, then `scripts/gen-api-types.py`. The second script needs `cargo install cargo-typify@0.8.0 --locked`.
5. Run the tests, and open a pull request. After it merges, the workflow updates or closes the drift issue.

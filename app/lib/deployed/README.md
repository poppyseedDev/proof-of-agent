# Deployed program interfaces

`devnet.json` is the IDL the program on devnet was built from. It is not the
IDL the app uses (`../idl.json`).

`npm test` compares the two (`tests/unit/idlCompat.test.ts`) and fails if the app's
IDL would not work against the deployed program. Replace this file with
`target/idl/proof_of_agent.json` right after upgrading the program on devnet, and
at no other time.

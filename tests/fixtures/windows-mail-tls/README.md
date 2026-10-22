This CA and server key are public, disposable test material for the loopback-only
Windows IMAP fixture. The server certificate covers only `127.0.0.1` / `localhost`.
The CA signing key was discarded. Never use these keys for real mail or install
the CA into a system trust store. The smoke process supplies `MAILPULSE_EXTRA_CA`
only to its isolated app process. Tests keep normal TLS verification enabled.

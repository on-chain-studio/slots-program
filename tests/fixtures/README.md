# Test fixtures

- `baseline.so` — the program as deployed: `SLoTSdnmBH5KtNJjhEYw1MeWTKAfRnFfQTTcpgwRn2Q` on devnet,
  last deployed in slot 502075720, dumped with `solana program dump` and cut at the end of its ELF
  (the account is zero-padded past it). sha256
  `8b2d6de681cf9e13a00c98904d23f12f1251eef30e79169c59c46c9816601cc9`. `tests/differential.rs`
  holds every build to it; replace it when a new build is deployed.
- `cpi_recorder.so` — built from `cpi-recorder/`; stands in for every program the game calls and
  logs what it was asked.

# Pairing-screen test

`tests/pairing.sh` reaches Zwift's device pairing screen without a Zwift account,
with the bluezoo Bluetooth mock running. It is a test aid; nothing here ships in the
snap.

```shell
workshop run zwift-snap pairing            # SMOKE_DISPLAY=auto|nested|headless, extra args go to yarf
```

The zwift snap must be installed and Zwift already downloaded into the prefix, so run
the [smoke test](../smoke/) once first.

## Where to look

- [../.kb/pairing-test.md](../.kb/pairing-test.md) — how it works: the fake login
  servers, the Bluetooth trainer mock, the pairing steps, artifacts and
  reproduction traces.
- [../.kb/real-auth-token-helper.md](../.kb/real-auth-token-helper.md) — requesting
  real `Game_Launcher` tokens with `tests/tools/zwift-token.py`.
- [../.kb/yarf-tooling.md](../.kb/yarf-tooling.md) — driving the running game by
  hand with `tests/tools/yarf/yarf.sh`.

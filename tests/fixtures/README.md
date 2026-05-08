# Test Fixtures

This directory holds WAV files used by the anyMic test suite.

## Auto-generated fixtures

These files are generated automatically by `conftest.py` if they are absent:

| File | Generator | Description |
|------|-----------|-------------|
| `chirp.wav` | `tools/gen_chirp.py` | 200 ms linear chirp 100 Hz → 8 kHz, 48 kHz s16 mono |
| `sweep.wav` | `tools/gen_sweep.py` | 5 s log sweep 20 Hz → 20 kHz, 48 kHz s16 mono |
| `speech.wav` | `tools/gen_speech.py` | 10 s synthetic vowel substitute, 48 kHz s16 mono |

Re-generate manually:

```bash
cd /path/to/anyMic/tests
python3 tools/gen_chirp.py  --out fixtures/chirp.wav
python3 tools/gen_sweep.py  --out fixtures/sweep.wav
python3 tools/gen_speech.py --out fixtures/speech.wav
```

## speech.wav — important note

`speech.wav` is a **synthetic substitute** produced by formant-filtered harmonic
vocoders.  It has speech-like energy distribution (80–4000 Hz) but no
intelligibility.  **It is not suitable for ITU-T P.862 (PESQ) evaluation.**

For accurate PESQ / MOS testing replace it with a real ITU-T P.50 reference:

- **ITU-T P.50 Appendix I** — "Artificial voices" available from the ITU:  
  <https://www.itu.int/rec/T-REC-P.50/en>
- **EBU SQAM disc** (track 70–71 are P.50 speech samples):  
  <https://tech.ebu.ch/publications/sqamcd>
- Any 48 kHz / 16-bit mono recording of P.50 material will work.

Drop the replacement file at `tests/fixtures/speech.wav` and the `fixture_speech`
pytest fixture will pick it up without modification.

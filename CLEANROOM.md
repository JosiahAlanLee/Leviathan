# Clean-Room Specification & Provenance Boundary

Leviathan is authored under the `BSD-3-Clause` license using a clean-room boundary:

1. **Permitted Normative Inputs (`BSD-3-Clause`):**
   - Score v3 structure, `1/4x`–`8x` pattern timing, `lowbias32` (`0x7feb352d` / `0x846ca68b`, salt `0x70726f62`), and `FILL` semantics as documented in `JosiahAlanLee/Gorgon/spec/`.
   - Control Voltage equations (`1V/oct` pitch with `pitch_zero = 60` or `48`, `Clock`, `Reset`, `Run`, `Gate`, `Velocity`, `5V`/`10V` automation, and `-5V..+5V` bipolar `roll`).
   - 9-channel `0V`/`10V` `ByteBus` word/clock encoding and the `336`-byte `GORGSND1` (`VERSION = 2`) shared-memory packet layout.

2. **Excluded Source Material:**
   - Zero source files, functions, or expressions are copied or translated from `AGPL-3.0-or-later` (`hydrapy`) or `GPL-3.0` (`VCV Rack` / `vcv/adapter`) repositories.
   - All Rust synthesis voices (`leviathan-dsp`), shader sources (`leviathan-gpu`), and native UI/audio shells (`leviathan-app`) are original clean-room implementations.

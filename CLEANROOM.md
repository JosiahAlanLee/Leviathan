# Clean-Room Specification & Provenance Boundary

Leviathan is authored under the `BSD-3-Clause` license using a clean-room boundary:

1. **Permitted Normative Inputs (`BSD-3-Clause`):**
   - Score v3 structure, `1/4x`–`8x` pattern timing, `lowbias32` (`0x7feb352d` / `0x846ca68b`, salt `0x70726f62`), and `FILL` semantics as documented in `JosiahAlanLee/Gorgon/spec/`.
   - Control Voltage equations (`1V/oct` pitch with `pitch_zero = 60` or `48`, `Clock`, `Reset`, `Run`, `Gate`, `Velocity`, `5V`/`10V` automation, and `-5V..+5V` bipolar `roll`).
   - 9-channel `0V`/`10V` `ByteBus` word/clock encoding and the `336`-byte `GORGSND1` (`VERSION = 2`) shared-memory packet layout.

2. **Excluded Source Material:**
   - Zero source files, functions, or expressions are copied or translated from `AGPL-3.0-or-later` (`hydrapy`) or `GPL-3.0` (`VCV Rack` / `vcv/adapter`) repositories.
   - All Rust synthesis voices (`leviathan-dsp`), shader sources (`leviathan-gpu`), and native UI/audio shells (`leviathan-app`) are original clean-room implementations.

3. **Wave 5 DSP Provenance**
   - PERC and WEST use the roadmap's behavioral descriptions, original oscillator and envelope equations, and Leviathan's existing DSP primitives. OTIS generates its own single-cycle tables from Fourier coefficients at compile time.
   - BEND uses the published [allpass quadrature topology](https://www.mikrocontroller.net/attachment/33904/hilbert_Olli_Niemitalo.pdf) with independently fitted coefficients and a [normalized orthogonal feedback network](https://www.di.univr.it/documenti/ArticoloRivista/allegato/allegato574545.pdf). The source inputs for Wave 5 were the roadmap, mathematical descriptions, and Leviathan's existing Rust code.
   - BBUS uses the [Parallel Sum behavior](https://github.com/JosiahAlanLee/DSP_Emulation/blob/a369a4a16898068be769622231241722dc674e0f/docs/NLC_BUS_SPEC.md#5-parallel-sum-from-burhans-timbre) and the [Adapter clock contract](https://github.com/JosiahAlanLee/DSP_Emulation/blob/a369a4a16898068be769622231241722dc674e0f/vcv/bytebus/docs/ADAPTER.md#clocks-startup-and-reset). The tracker bridge, register injection, FIR morph, and ladder transfer and transient are original Rust derived from those behaviors and the T21 requirements.
   - OTIS Twin uses the supplied T24 Rust prototype, the behavioral references in the roadmap, and Leviathan's DSP primitives. It generates its shared 16 KiB tables from mathematical waveforms.

4. **Wave 6 DSP Provenance**
   - T25 uses the supplied Rust prototype and the converter behavior described in the roadmap. Yamaha's datasheet supplies the YM3012 serial fields. The CZ transfer is a provisional mathematical model.
   - T26 uses the supplied FS77 Rust prototype, Yamaha's SY77 and FS1R behavioral descriptions, and Leviathan DSP primitives. The waveform modulator reads Leviathan's generated OTIS table.
   - T27 uses the supplied K26 Rust prototype, the Kurzweil behavioral references in the roadmap, and Leviathan DSP primitives. The three-stage routing, oscillator and filter voicing, and recursive modulation are Leviathan's implementation. The keymap source reads Leviathan's generated OTIS tables.

5. **Wave 7 DSP Provenance**
   - T28–T30 use the capture, frame scanning, and audio input behavior linked in the roadmap. They reuse Leviathan's clock, mixer, OTIS voices, and VAST algorithms with a new project sample source.
   - T31 uses the clock and reset contract in the [ByteBus Register manual](https://github.com/JosiahAlanLee/DSP_Emulation/blob/a369a4a16898068be769622231241722dc674e0f/vcv/bytebus/docs/MANUAL.md) and the identity and inversion anchors in the [Lookup manual](https://github.com/JosiahAlanLee/DSP_Emulation/blob/a369a4a16898068be769622231241722dc674e0f/vcv/bytebus/docs/LOOKUP.md). The tracker routing and maps are original Rust. Carbon's standalone draft informed the scope review.

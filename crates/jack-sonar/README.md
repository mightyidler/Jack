# Jack Sonar adapter

In-process HTTP control of an already installed and running SteelSeries GG/Sonar.
Discovery is `PROGRAMDATA/coreProps.json → GG /subApps → Sonar`, never a fixed port.
The protocol modules and error model are adapted from [adrifer/sonarctl](https://github.com/adrifer/sonarctl),
commit `df6dd47d38f0f5271c074c5ccb6bfb052af7fd9e` (MIT, copyright Adrian Fernandez 2026).
The license is preserved in `LICENSE-sonarctl` and bundled with the desktop assets.
No CLI subprocesses or C# runtime are used. Jack's service owns mode gating, readback,
conflict checks, loopback restrictions, retry state and application identity checks.

Supported: Classic channel/master/microphone volume and mute, physical device routing,
PID-based live output-session assignment. Streamer mode is detected and safely blocked.
EQ, ChatMix, presets, custom channel creation and automatic app classification are not implemented.
Sonar retains audio state when Jack closes; switching Jack's channel view off does not undo Sonar routes.

`JACK_SONAR_CORE_PROPS` optionally overrides the discovery file for testing installations.
All discovered addresses remain restricted to localhost, 127.0.0.1 or ::1.

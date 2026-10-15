# 개발·배포

Windows 11 x64, Rust stable MSVC, Visual Studio C++ Build Tools, Node.js 24가 필요하다.

```powershell
cd desktop
npm ci
npm run tauri -- dev
```

소스 검사:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd desktop
npm run build
```

오디오·화면을 조작하는 진단 도구는 일반 검사에서 실행하지 않는다.

설치 파일 빌드:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = '저장소 밖의 Jack 전용 키 경로'
cd desktop
npm run tauri -- build --bundles nsis
```

결과는 `target/release/bundle/nsis`에 생성된다. 업데이트 서명은 Windows Authenticode 서명과 별개다. 키를 잃으면 기존 앱에 같은 키로 업데이트를 제공할 수 없다.

GitHub Secrets에 `TAURI_SIGNING_PRIVATE_KEY`를 등록한다. 암호가 있는 키는 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`도 등록한다. 공개 키는 `desktop/src-tauri/tauri.conf.json`에 둔다.

버전은 루트 `Cargo.toml`, `desktop/package.json`·lock, `tauri.conf.json`에서 일치시킨다. `v버전` 태그를 푸시하면 설치 파일·서명·`latest.json`을 포함한 초안 릴리스를 만든다. 사용자 확인을 마친 뒤 공개한다.

GG 최신 조회는 공식 최신 주소의 HTTPS 리다이렉트와 파일명에서 버전을 확인한다. 응답이 바뀌면 확인 실패로 처리한다. 설치에는 유효한 Authenticode 서명과 허용된 게시자(SteelSeries ApS/A/S 또는 GN Hearing A/S)가 필요하다. 자동 무인 설치는 사용하지 않는다.

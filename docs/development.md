# 개발·배포

Windows 11 x64, Rust 1.99.1 MSVC, Visual Studio C++ Build Tools, Node.js 26.12.0, npm 11.20.0이 필요하다. Node.js 버전은 루트 `.node-version`, Rust 버전은 `rust-toolchain.toml`에 고정하며 CI도 같은 버전을 사용한다.

UI만 테스트할 때는 Node.js와 npm만 있으면 된다. Rust 빌드·설치·릴리즈 없이 다음 명령으로 미리보기 선택 페이지를 연다.

```powershell
cd desktop
npm ci
npm run dev:ui
```

`http://127.0.0.1:1421/ui.html`에서 출력·마이크·설정·트레이 화면, 테마, 언어, 장치 상태와 그룹 수를 선택한다. 소스를 저장하면 Vite가 즉시 반영한다. 볼륨·장치 선택·앱 이동·그룹 추가/삭제·설정·단축키는 메모리의 샘플 데이터만 바꾸며, 미리보기 초기화로 원래 상태로 돌아간다. 실제 오디오 제어, GG 연결, 업데이트, 시작 프로그램 등록, 네이티브 창 배치는 아래 개발 앱에서 확인한다.

입력 장치별 볼륨은 전체 Sonar 장치 ID를 기준으로 기억하고 장치 전환 시 복원한다. Sonar는 선택된 입력의 믹서 볼륨만 제공하므로, 저장값이 없는 입력들은 장치를 처음 발견한 시점의 믹서 볼륨을 초기값으로 각각 기록한다. 이후 선택된 입력의 볼륨 변경은 다른 입력의 초기값을 바꾸지 않는다. 네이티브 앱은 설정 파일에 저장하며, 웹 미리보기는 현재 페이지의 메모리에만 저장하므로 새로고침·초기화 시 사라진다.

네이티브 개발 앱 실행:

```powershell
cd desktop
npm ci
npm run tauri -- dev
```

F12 / Ctrl+Shift+I 개발자 도구는 네이티브 디버그 모드에서만 허용한다. 릴리즈에서는 Tauri `devtools` 기능을 포함하지 않고 `open_devtools` 호출도 거절한다. 브라우저 UI 테스트에서는 브라우저 개발자 도구를 사용할 수 있다.

소스 검사:

```powershell
node tools/collect-licenses.mjs --check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd desktop
npm run lint
npm test
npm run build
```

오디오·화면을 조작하는 진단 도구는 일반 검사에서 실행하지 않는다.

프런트엔드 정적 폰트의 원본과 생성 기록은 `desktop/font-sources`에 보관한다. 앱에는 `desktop/public/assets/fonts`의 정적 폰트만 포함한다. 아이콘은 Windows NSIS 배포에 사용하는 파일과 재생성용 원본 `desktop/src-tauri/icons/icon.png`를 보관한다.

의존성을 바꾸면 `node tools/collect-licenses.mjs`로 원문 고지를 갱신한다. 레지스트리 패키지에 없는 원문은 `licenses/vendor/sources.json`의 출처와 함께 보존한다.

설치 파일 빌드:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = '저장소 밖의 Jack 전용 키 경로'
cd desktop
npm run tauri -- build --bundles nsis
```

결과는 `target/release/bundle/nsis`에 생성된다. 업데이트 서명은 Windows Authenticode 서명과 별개다. 키를 잃으면 기존 앱에 같은 키로 업데이트를 제공할 수 없다.

GitHub Secrets에 `TAURI_SIGNING_PRIVATE_KEY`를 등록한다. 암호가 있는 키는 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`도 등록한다. 공개 키는 `desktop/src-tauri/tauri.conf.json`에 둔다.

버전은 루트 `Cargo.toml`·`Cargo.lock`, `desktop/package.json`·lock, `tauri.conf.json`에서 일치시킨다. 릴리즈 노트는 `docs/release-notes.md`에서 갱신한다. `v버전` 태그를 푸시하면 소스·단위 검사와 프런트엔드 빌드/CSS 검사를 필수 실행하고 서명된 NSIS 설치 파일을 만든다. 기존 공개 버전과 같거나 낮은 버전의 재배포는 차단한다.

배포 작업은 초안의 설치 파일·서명·`latest.json`을 다시 내려받아 버전·주소·플랫폼·서명을 대조한다. Rust 업데이터의 실제 manifest 파서와 기존 0.1.4·0.1.5 공개 키로 호환성을 확인하고 변조된 바이트의 거부도 검사한다. 통과하면 정식 공개하면서 Latest로 지정하고, 인증 없는 `releases/latest/download/latest.json`과 설치 파일·서명을 다시 내려받아 검증한다. 초안만 생성한 상태나 Actions의 빌드 성공만으로 배포 완료로 간주하지 않는다. 공개 후 확인 단계만 실패한 경우 파일을 재빌드하거나 덮어쓰지 말고 `node tools/verify-release.mjs --published artifacts/distribution/public`으로 확인을 재실행한다. 실제 앱 내 설치·재시작·오디오 확인은 별도이며 배포 기록에서 구분한다.

GG 최신 조회는 공식 최신 주소의 HTTPS 리다이렉트와 파일명에서 버전을 확인한다. 응답이 바뀌면 확인 실패로 처리한다. 설치에는 유효한 Authenticode 서명과 허용된 게시자(SteelSeries ApS/A/S 또는 GN Hearing A/S)가 필요하다. 자동 무인 설치는 사용하지 않는다.

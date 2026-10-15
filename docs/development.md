# 개발·배포

Windows 11 x64, Rust 1.95.0 MSVC, Visual Studio C++ Build Tools, Node.js 24가 필요하다.

UI만 테스트할 때는 Node.js 24만 있으면 된다. Rust 빌드·설치·릴리즈 없이 다음 명령으로 미리보기 선택 페이지를 연다.

```powershell
cd desktop
npm ci
npm run dev:ui
```

`http://127.0.0.1:1421/ui.html`에서 출력·마이크·설정·트레이 화면, 테마, 언어, 장치 상태와 그룹 수를 선택한다. 소스를 저장하면 Vite가 즉시 반영한다. 볼륨·장치 선택·앱 이동·그룹 추가/삭제·설정·단축키는 메모리의 샘플 데이터만 바꾸며, 미리보기 초기화로 원래 상태로 돌아간다. 실제 오디오 제어, GG 연결, 업데이트, 시작 프로그램 등록, 네이티브 창 배치는 아래 개발 앱에서 확인한다.

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

버전은 루트 `Cargo.toml`, `desktop/package.json`·lock, `tauri.conf.json`에서 일치시킨다. `v버전` 태그를 푸시하면 설치 파일·서명·`latest.json`을 포함한 초안 릴리스를 만든다. 기본 배포는 패키징에 필요한 검사만 수행한다. 전체 소스·단위 검사는 Actions 수동 실행의 `run_checks` 옵션으로 선택한다. 실제 PC 확인은 별도이며, 배포 시 확인 범위를 기록한다.

GG 최신 조회는 공식 최신 주소의 HTTPS 리다이렉트와 파일명에서 버전을 확인한다. 응답이 바뀌면 확인 실패로 처리한다. 설치에는 유효한 Authenticode 서명과 허용된 게시자(SteelSeries ApS/A/S 또는 GN Hearing A/S)가 필요하다. 자동 무인 설치는 사용하지 않는다.

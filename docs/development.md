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

출력 장치 동기화를 OFF에서 ON으로 바꾸면 모든 오디오 그룹이 게이밍 그룹의 현재 출력 장치를 사용한다. 장치가 바뀌는 그룹은 해당 장치·그룹 조합에 저장된 볼륨(0% 포함)을 복원하며, 저장값이 없거나 이미 같은 장치를 쓰는 그룹은 현재 볼륨을 유지한다. 마이크는 독립적으로 유지한다. 장치·볼륨 적용 또는 설정 저장에 실패하면 이전 상태로 복구하고 동기화는 OFF를 유지한다.

볼륨 행 위에서 휠은 5%, Shift+휠은 1%씩 조절한다. 작은 트랙패드 이동량은 누적하고 연속 입력은 기존 라이브 볼륨 경로에서 병합한다. 선택되지 않은 마이크·연결 해제된 출력에는 적용하지 않는다. 출력 연결 해제는 실제 0%와 별도로 표시하며 마지막 장치별 볼륨을 유지한다. 장치의 USB·드라이버 설명은 표시 이름에서 제외하고 Speakers·Headphones 같은 짧은 이름으로 구별한다. 짧은 이름도 같으면 전체 ID 순서에 따른 번호를 붙인다. 연결 해제된 출력의 아이콘과 이름은 흐리게 표시하지만 장치 선택 쉐브론은 활성 상태를 유지한다. 툴팁에는 원본 이름을 보존한다.

외부 볼륨 변경은 기존 DOM과 볼륨 조작의 기준값만 갱신한다. 앱 목록 변경은 영향을 받는 그룹의 목록만, 출력·입력 장치 변경은 해당 컨트롤만 다시 구성한다. 메인과 트레이에 공통 적용하며 그룹·언어·테마 등 화면 구성이 달라지면 일반 렌더 경로를 사용한다. 오디오 IPC 실패는 `code`와 기술 상세 `detail`을 반환하며 화면은 한국어·영어 해결 안내를 표시한다. 기존 문자열 이벤트도 공통 오류 변환기를 거친다.

`desktop/tools/audio-quality-ui-smoke.cjs`는 실행 중인 웹 미리보기(기본 1421)에 모의 IPC를 연결해 휠·외부 변경·DOM 유지·앱 이동·장치 연결 해제·마이크·오류 안내를 검사한다. 실제 오디오는 변경하지 않는다. Playwright가 별도 경로에 있다면 `PLAYWRIGHT_MODULE`을 지정한다. `audio-quality-native-smoke.cjs`는 개발 앱의 loopback WebView2 CDP 포트 9223에 연결하는 별도 수동 실행 도구다. 실제 게이밍 볼륨만 잠시 변경하고, 외부 변경이 없을 때 원래 값으로 복원한다. 일반 단위 검사에는 포함하지 않는다.

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

Tauri Action이 `latest.json`에 API asset URL을 생성하면 인증된 릴리즈 메타데이터의 설치 파일 ID와 대조한 뒤 공개 `releases/download/v버전/설치파일` 주소로 변환한다. 서명은 그대로 유지하고 수정한 manifest를 다시 내려받아 검사한다. 서명 빌드까지 성공한 초안에서 배포 메타데이터만 수정할 때는 Actions의 `reuse_draft`를 사용할 수 있다. 이때 main의 배포 도구를 사용하되 앱·의존성·툴체인·라이선스 소스가 태그와 정확히 같아야 하며, 전체 검사와 서명/공개 검증을 다시 통과해야 한다. 이미 생성된 설치 파일과 서명은 재빌드하거나 교체하지 않는다.

GG 최신 조회는 공식 최신 주소의 HTTPS 리다이렉트와 파일명에서 버전을 확인한다. 응답이 바뀌면 확인 실패로 처리한다. 설치에는 유효한 Authenticode 서명과 허용된 게시자(SteelSeries ApS/A/S 또는 GN Hearing A/S)가 필요하다. 자동 무인 설치는 사용하지 않는다.

# 배포 작업 상태

목표: Windows 11에서 Jack 설치, GG/Sonar 설정, GitHub Releases 업데이트를 연결한다.

- 첫 배포는 GG/Sonar Classic 필수다. 기존 GG 설치와 사용자 설정을 재사용한다.
- 공식 최신 주소: https://steelseries.com/gg/downloads/gg/latest/windows
- 2026-10-15 확인: 공식 주소가 GG 121.0.0으로 연결된다. 제품에는 버전을 고정하지 않는다.
- GG의 공식 대화형 설치를 사용한다. 무인 설치·약관 생략은 사용하지 않는다.
- Jack 업데이트는 GG 설치·업데이트와 별도로 처리한다.
- Bald 유래 코드는 GPL-3.0 라이선스를 확인했다. Sonar 어댑터의 MIT 고지도 유지한다.
- 공개 저장소: https://github.com/mightyidler/Jack . 기존 Git 인증을 사용한다.
- Jack 전용 업데이트 서명 키를 생성하고 GitHub Secrets에 등록했다. 비밀 키는 소스에 포함하지 않는다.
- 공식 GG 설치 파일의 제품명·버전·유효한 GN Hearing A/S 서명을 읽기 전용으로 확인했다.
- Rust Clippy, 단위 검사 116개(도우미 1개 제외), TypeScript/Vite 빌드, GG 설치 방어 규칙 검사를 통과했다.
- 로컬 NSIS 설치 파일과 업데이트 서명을 생성했다. Windows Authenticode 코드 서명은 적용하지 않았다.
- 설치 파일의 업데이트 서명을 앱 공개 키로 검증했고, 바이트 변조 거부도 확인했다.

실제 설치·화면·오디오 검증은 아직 수행하지 않았다. 빌드·패키지 검사와 별도로 기록한다.

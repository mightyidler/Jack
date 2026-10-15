# 배포 상태

- 공개 저장소: https://github.com/mightyidler/Jack
- 기존 GG 설치를 재사용한다. 공식 최신 주소는 https://steelseries.com/gg/downloads/gg/latest/windows 이다.
- 공식 최신 GG 121.0.0의 제품 정보와 유효한 GN Hearing A/S 서명을 확인했다(2026-10-15).
- 사용자 확인: 기존 GG로 설치·연결·출력·마이크 정상. 0.1.1 시작 실행 스위치·Bald 업데이트 행·토스트 정상.
- 소스·빌드·업데이트 서명·변조 거부·압축 CSS 타이밍 검사를 통과했다. 실제 오디오·화면 자동 검사는 실행하지 않았다.
- Jack 전용 업데이트 비밀 키는 소스 밖과 GitHub Secrets에 둔다. Windows Authenticode 코드 서명은 적용하지 않았다.
- 최초 CI의 Rust 버전 차이를 해결하기 위해 1.95.0으로 고정했다. 0.1.2는 설치된 0.1.1의 온라인 업데이트 확인용 첫 배포판이다.
- GG 미설치·구버전·재부팅 등 다른 환경의 실제 설치 확인과 온라인 업데이트 확인은 별도다.

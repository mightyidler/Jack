// Format presentation only. Endpoint IDs and original names remain untouched.
const genericRole = /^(?:(?:\d+\s*[-–]\s*)?)(?:speakers?|headphones?|headset(?:\s+(?:earphone|microphone))?|earphones?|microphones?|mic|line[ -]?(?:in|out)|digital(?:\s+audio)?(?:\s*\(s\/pdif\))?|audio(?:\s+(?:input|output))?|스피커|헤드폰|헤드셋|이어폰|마이크|라인\s*(?:입력|출력)|디지털\s*(?:오디오|출력)|오디오\s*(?:입력|출력))$/i;
const driverDescription = /(?:high\s+definition\s+audio|고음질\s*오디오|(?:display|hdmi|usb|bluetooth|wireless|generic)\s+audio(?:\s+(?:device|driver))?|audio\s+(?:driver|controller))$/i;

export function deviceLabel(raw: string): string {
  let name = raw.trim();
  // Peel balanced trailing wrappers, including nested model parentheses.
  for (let pass = 0; pass < 8; pass++) {
    const closing = name.endsWith(')') ? ')' : name.endsWith('）') ? '）' : '';
    if (!closing) break;
    const opening = closing === ')' ? '(' : '（';
    let depth = 0, start = -1;
    for (let i = name.length - 1; i >= 0; i--) {
      if (name[i] === closing) depth++;
      else if (name[i] === opening && --depth === 0) { start = i; break; }
    }
    if (start < 0) break;
    const prefix = name.slice(0, start).trim();
    const inner = name.slice(start + 1, -1).trim();
    if (!inner) break;
    if (!prefix || genericRole.test(prefix)) name = inner;
    else if (driverDescription.test(inner)) name = prefix;
    else break; // Preserve meaningful model qualifiers rather than guessing.
  }
  return name || raw;
}

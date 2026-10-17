// Format presentation only. Endpoint IDs and original names remain untouched.
const genericRole = /^(?:(?:\d+\s*[-–]\s*)?)(?:speakers?|headphones?|headset(?:\s+(?:earphone|microphone))?|earphones?|microphones?|mic|line[ -]?(?:in|out)|digital(?:\s+audio)?(?:\s*\(s\/pdif\))?|audio(?:\s+(?:input|output))?|스피커|헤드폰|헤드셋|이어폰|마이크|라인\s*(?:입력|출력)|디지털\s*(?:오디오|출력)|오디오\s*(?:입력|출력))$/i;
const driverDescription = /(?:high\s+definition\s+audio|고음질\s*오디오|(?:display|hdmi|usb|bluetooth|wireless|generic)\s+audio(?:\s+(?:device|driver))?|audio\s+(?:driver|controller))$/i;
const transportDescription = /^(?:usb(?:[- ]?\d+(?:\.\d+)?)?|bluetooth|wireless|hdmi|displayport|generic)$/i;

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
    if (prefix && genericRole.test(prefix) && (transportDescription.test(inner) || driverDescription.test(inner))) name = prefix;
    else if (!prefix || genericRole.test(prefix)) name = inner;
    else if (driverDescription.test(inner)) name = prefix;
    else break; // Preserve meaningful model qualifiers rather than guessing.
  }
  return name || raw;
}

// Keep picker labels short; only tooltips retain transport/driver details.
export function deviceLabels(devices:readonly {id:string;name:string;role:string}[]):Map<string,string> {
  const labels=new Map(devices.map(device=>[device.id,deviceLabel(device.name)]));
  for(const device of devices) {
    const peers=devices.filter(other=>other.role===device.role && deviceLabel(other.name)===deviceLabel(device.name));
    if(peers.length<2)continue;
    const short=(name:string)=>name.replace(/\s*[(（].*$/u,'').trim()||deviceLabel(name);
    const label=short(device.name);
    const duplicates=peers.filter(other=>short(other.name)===label).sort((a,b)=>a.id.localeCompare(b.id));
    labels.set(device.id,label+(duplicates.length>1?` · ${duplicates.findIndex(other=>other.id===device.id)+1}`:''));
  }
  return labels;
}

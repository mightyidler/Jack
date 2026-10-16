import type { ApplicationGroup } from './application-groups';

type Orders=Record<string,string[]>;
const channels=['game','chat','media','aux','other'];

export function applicationOrderKey(application:ApplicationGroup,identities:ReadonlyMap<number,{stable_id:string|null}>) {
  const stable=application.process_ids.map(pid=>identities.get(pid)?.stable_id).find(Boolean);
  return stable?JSON.stringify(['app',stable]):JSON.stringify(['process',application.session.process_id]);
}

// Presentation order is independent of the audio inventory (and of process IDs
// for verified apps). Keep absent apps so restarting one retains its position.
export class ApplicationOrder {
  private orders:Orders={};

  constructor(saved:string|null=null) {
    try {
      const value:unknown=JSON.parse(saved??'null');
      if(!value || typeof value!=='object')return;
      for(const channel of channels) {
        const keys=(value as Orders)[channel];
        if(Array.isArray(keys))this.orders[channel]=[...new Set(keys.filter(key=>typeof key==='string'))];
      }
    } catch { /* A damaged preference must not hide applications. */ }
  }

  sort<T>(channel:string,items:readonly T[],key:(item:T)=>string):T[] {
    const ranks=new Map((this.orders[channel]??[]).map((id,index)=>[id,index]));
    return [...items].sort((a,b)=>(ranks.get(key(a))??ranks.size)-(ranks.get(key(b))??ranks.size));
  }

  place(channel:string,key:string,before:string|null,visible:readonly string[]):()=>void {
    const previous=this.orders;
    const next:Orders=Object.fromEntries(Object.entries(previous).map(([name,keys])=>[name,keys.filter(id=>id!==key)]));
    const keys=next[channel]??[];
    for(const id of visible)if(id!==key && !keys.includes(id))keys.push(id);
    const index=before===null?-1:keys.indexOf(before);
    keys.splice(index<0?keys.length:index,0,key);
    next[channel]=keys;this.orders=next;
    return ()=>{this.orders=previous;};
  }

  serialize():string {return JSON.stringify(this.orders);}
}

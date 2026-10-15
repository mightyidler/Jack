// Share an outstanding read without queuing another copy on slow native I/O.
export function singleFlight<A extends unknown[], T>(read:(...args:A)=>Promise<T>) {
  let flight:Promise<T>|null=null;
  return (...args:A):Promise<T> => {
    if(!flight)flight=Promise.resolve().then(()=>read(...args)).finally(()=>{flight=null;});
    return flight;
  };
}

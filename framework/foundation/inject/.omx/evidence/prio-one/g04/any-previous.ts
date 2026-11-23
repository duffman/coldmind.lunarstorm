import type { IContainer } from '@coldmind/inject';
type PreviousFactory<T, D = void> = D extends void ? (c: IContainer) => T | Promise<T> : (c: IContainer, data: D) => T | Promise<T>;
declare const c: IContainer;
declare const declared: PreviousFactory<number, any>;
const required: PreviousFactory<number, any> = (_c, data: any) => data.value;
const ignoring: PreviousFactory<number, any> = () => 1;
declared(c, {}); required(c, {}); ignoring(c, {});
declared(c); required(c); ignoring(c);

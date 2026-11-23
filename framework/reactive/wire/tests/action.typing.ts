// Compile-only contracts. This file is intentionally not a runtime test.
import { Action } from '@coldmind/wire';

function expectType<T>(_value: T): void {}

const numbers = new Action<number>();
numbers.onData(value => expectType<number>(value));
numbers.emit(1);
// @ts-expect-error A channel cannot accept a different payload type.
numbers.emit('1');
// @ts-expect-error Non-void actions require a payload.
numbers.emit();
// @ts-expect-error onData cannot override the channel's payload type.
numbers.onData<string>(() => {});
// @ts-expect-error Listener input must match the channel.
numbers.onData((value: string) => value);
// @ts-expect-error A synchronous filter cannot accept a promise.
numbers.filter(async () => true);

expectType<Action<string>>(numbers.map(String));
expectType<Action<number>>(numbers.debounce(10).distinct());
expectType<Action<string>>(numbers.switchMap(async value => String(value)));
expectType<Promise<number>>(numbers.next());
expectType<Action<number>>(Action.fromPromise(Promise.resolve(1)));
expectType<Action<never>>(Action.merge());

const optional = new Action<string | undefined>();
expectType<Action<string>>(optional.filter((value): value is string => value !== undefined));
expectType<Action<number | string | boolean>>(Action.merge(numbers, new Action<string>(), new Action<boolean>()));
expectType<Action<number | string | boolean>>(numbers.merge(new Action<string>(), new Action<boolean>()));
// @ts-expect-error Connected sources must produce the receiver's payload type.
numbers.connect(new Action<string>());
new Action<void>().emit();
// @ts-expect-error A callback may return synchronous cleanup, not a promise of cleanup.
Action.fromCallback<number>(async () => () => {});

declare const button: HTMLButtonElement;
declare const document: Document;
Action.fromEvent(button, 'click').onData(event => expectType<number>(event.clientX));
Action.fromEvent(document, 'keydown').onData(event => expectType<string>(event.key));
new Action<number>().fromEvent(button, 'click', event => event.clientX);
new Action<string>().fromEvent(document, 'keydown', event => event.key);
Action.fromEvent<CustomEvent<{ id: number }>>(new EventTarget(), 'user')
  .onData(event => expectType<number>(event.detail.id));
// @ts-expect-error A mapped event must produce the Action's payload type.
new Action<number>().fromEvent(button, 'click', () => 'wrong');

import { Action } from '@coldmind/wire';

interface User {
  readonly id: number;
  readonly name: string;
}

const users = new Action<User>();
users.onError(error => console.error('Users:', error));
users.onData(user => console.log('User:', user.name));
users.emit({ id: 1, name: 'Alice' });

const promised = users.next();
users.fromPromise(Promise.resolve({ id: 2, name: 'Bob' }));
await promised;

const updates = new EventTarget();
users.fromEvent<CustomEvent<User>>(updates, 'user', event => event.detail);
updates.dispatchEvent(new CustomEvent<User>('user', { detail: { id: 3, name: 'Charlie' } }));

const ticks = Action.fromCallback<number>(emit => {
  let count = 0;
  const timer = setInterval(() => emit(++count), 5);
  return () => clearInterval(timer);
});
console.log('First tick:', await ticks.next({ timeout: 1_000 }));
// next() removed the only observer, so the interval has already been cleared.
ticks.dispose();

const catalog: readonly User[] = [
  { id: 1, name: 'Alice' },
  { id: 2, name: 'Bob' },
  { id: 3, name: 'Charlie' },
];

const search = new Action<string>();
const results = search
  .map(text => text.trim())
  .filter(text => text.length >= 2)
  .debounce(20)
  .distinct()
  .switchMap(async (query, signal) => {
    signal.throwIfAborted();
    return catalog.filter(user => user.name.toLowerCase().includes(query.toLowerCase()));
  });

results.onError(error => console.error('Search:', error));
results.onData(matches => console.log('Matches:', matches.map(user => user.name).join(', ')));
const found = results.next({ timeout: 1_000 });
search.emit('Al').emit(' Ali ').emit(' Alice ');
await found;

const saved = new Action<User>();
const removed = new Action<User>();
const changes = Action.merge(saved, removed);
changes.onData(user => console.log('Changed:', user.name));
saved.emit(catalog[0]!);
removed.emit(catalog[1]!);

changes.dispose();
saved.dispose();
removed.dispose();
search.dispose();
users.dispose();

// One setImmediate chain on its own: each callback queues the next, with no I/O and
// no timers in the way, so the process spends its time in loop turns.
// profile-turn.sh samples this process while it runs.
let turns = 0;
const spin = () => {
  turns += 1;
  setImmediate(spin);
};
setImmediate(spin);
setTimeout(() => {
  console.log('turns', turns);
  process.exit(0);
}, 10000);

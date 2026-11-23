// Exercises the tao shell end to end without a person: opens a window, retitles
// it, closes it from a timer, and lets the last window's close end the process.
import { serve } from '@desktop/bridge';
import { native } from '@desktop/api';

serve((message) => {
  if (message.event === 'onClose') console.log(`${message.form} closed`);
});

await native.window.create('Main', { title: 'selfcheck', width: 320, height: 200 });
console.log('created');
await native.window.setTitle('Main', 'selfcheck: retitled');
console.log('retitled');
try {
  await native.window.setTitle('Missing', 'x');
} catch (error) {
  console.log(`refused: ${error.message}`);
}
try {
  await native.dialog.open();
} catch (error) {
  console.log(`refused: ${error.message}`);
}
setTimeout(() => native.window.close('Main'), 300);

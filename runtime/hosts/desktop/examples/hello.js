// A desktop script: opens a native window and reacts to it.
//
//   lunarstorm-desktop examples/hello.js
//
// The window lives on the main thread, in tao's event loop; this script runs on
// the backend thread, in the Lunarstorm Runtime. `native.*` calls go to the main
// thread and window events come back through `serve`.
import { serve } from '@desktop/bridge';
import { native } from '@desktop/api';

serve((message) => {
  switch (message.event) {
    case 'onResize':
      native.window.setTitle(message.form, `Hello from Lunarstorm: ${message.payload.width} x ${message.payload.height}`);
      break;
    case 'onCloseQuery':
      // Return { cancel: true } to keep the window open.
      return { cancel: false };
    case 'onClose':
      console.log(`${message.form} closed`);
      break;
  }
});

await native.window.create('Main', { title: 'Hello from Lunarstorm', width: 640, height: 400 });
console.log('Main is open; close it to quit');

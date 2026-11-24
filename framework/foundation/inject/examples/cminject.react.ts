import { Container, createToken } from '@coldmind/inject';
import { ContainerProvider, useScope, useService } from '@coldmind/inject/react';
import { createElement } from 'react';
import { renderToString } from 'react-dom/server';

/** Prepares a request scope before synchronous server rendering and disposes it afterwards. */
export async function runExample(): Promise<string> {
  const Greeting = createToken<{ text: string }>('Greeting');
  const root = new Container();
  let disposed = 0;
  let markup = '';
  try {
    root.request(Greeting, {
      factory: async () => { await Promise.resolve(); return { text: 'Hello Ada' }; },
      onDispose: () => { disposed++; },
    }, { deps: [] });
    const request = root.createRequestScope('render');
    try {
      await request.resolve(Greeting);
      function Message() {
        const greeting = useService(Greeting);
        const scope = useScope();
        if (scope !== request) throw new Error('Wrong SSR scope');
        return createElement('p', null, greeting.text);
      }
      markup = renderToString(createElement(ContainerProvider, { container: request }, createElement(Message)));
    } finally {
      await request.dispose();
    }
  } finally {
    await root.dispose();
  }
  return `${markup}|disposed:${disposed}`;
}

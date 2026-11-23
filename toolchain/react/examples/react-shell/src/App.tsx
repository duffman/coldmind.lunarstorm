import { useState } from "react";

import { useShellChannel, useShellStatus } from "@coldmind/devtools-shell";

/** Edit this file while the server runs: state survives, and a thrown render error recovers on save. */
export default function App() {
  const [count, setCount] = useState(0);
  const [echoes, setEchoes] = useState<string[]>([]);
  const status = useShellStatus();
  const send = useShellChannel<string>("echo", (text) => setEchoes((list) => [...list, text]));

  return (
    <main>
      <h1>devtools shell</h1>
      <p>socket: {status}</p>
      <button type="button" onClick={() => setCount((n) => n + 1)}>
        clicked {count}
      </button>
      <button type="button" onClick={() => send(`hello ${count}`)}>
        send on app:echo
      </button>
      <ul>
        {echoes.map((text, i) => (
          <li key={i}>{text}</li>
        ))}
      </ul>
    </main>
  );
}

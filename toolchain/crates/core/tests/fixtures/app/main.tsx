import { greet } from "./greet.ts";

interface Props {
  name: string;
}

function Hello({ name }: Props) {
  return <p>{greet(name)}</p>;
}

const element = <Hello name="devtools" />;
console.log(JSON.stringify({ type: typeof element.type, props: element.props }));

export default (
  <form name="Main" caption="Lunarstorm">
    <panel name="Top" align="top" layout="row" gap={8}>
      <label name="Greeting" caption="Hello" />
      <button name="Greet" caption="Greet" onClick={form.greet} />
    </panel>
  </form>
);

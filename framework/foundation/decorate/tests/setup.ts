const expected = process.env.DECORATE_EXPECT_RUNTIME;
if (expected === 'bun' && !process.versions.bun) throw new Error('Decorate tests must execute in a Bun worker');
if (expected === 'node' && process.versions.bun) throw new Error('Decorate tests must execute in a Node worker');

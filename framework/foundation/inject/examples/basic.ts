import { Container, ServiceLifetime } from '@coldmind/inject';
import { DiRuntime } from '@coldmind/inject/server';

class Repository {
  static $injectable = { lifetime: ServiceLifetime.Singleton };
  async read(id: string): Promise<{ id: string }> { return { id }; }
}
class Controller {
  static $injectable = { lifetime: ServiceLifetime.Request };
  static $inject = [Repository];
  constructor(readonly repository: Repository) {}
  async get(id: string): Promise<{ id: string }> { return this.repository.read(id); }
}

const container = new Container();
container.registerClass(Repository);
container.registerClass(Controller);
container.validate();

const runtime = new DiRuntime<string>(container);
await runtime.start();
try {
  console.log(await runtime.withRequest('tenant-alpha', async request => {
    const controller = await request.resolve(Controller);
    return controller.get('article-42');
  }));
} finally {
  await runtime.dispose();
}

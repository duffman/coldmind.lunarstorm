import { Container, createToken, Inject, Injectable, ServiceLifetime } from '@coldmind/inject';

/** Uses standard syntax, explicitly typed legacy applications and static declarations. */
export async function runExample(): Promise<string> {
  const Name = createToken<string>('Name');
  @Injectable({ lifetime: ServiceLifetime.Scoped, deps: [Name] })
  class StandardGreeting {
    @Inject(Name) suffix!: string;
    constructor(readonly name: string) {}
    text(): string { return `${this.name}:${this.suffix}`; }
  }
  class LegacyGreeting {
    suffix!: string;
    constructor(readonly name: string) {}
    text(): string { return `${this.name}:${this.suffix}`; }
  }
  const legacyClass: ClassDecorator = Injectable({ lifetime: ServiceLifetime.Scoped });
  const legacyParameter: (target: typeof LegacyGreeting, key: undefined, index: number) => void = Inject(Name);
  const legacyField: PropertyDecorator = Inject(Name);
  legacyClass(LegacyGreeting);
  legacyParameter(LegacyGreeting, undefined, 0);
  legacyField(LegacyGreeting.prototype, 'suffix');
  class StaticGreeting {
    static $injectable = { lifetime: ServiceLifetime.Scoped };
    static $inject = [Name];
    static $members = { suffix: Name };
    suffix!: string;
    constructor(readonly name: string) {}
    text(): string { return `${this.name}:${this.suffix}`; }
  }
  const root = new Container();
  try {
    root.singleton(Name, () => 'Ada', { deps: [] });
    root.registerClass(StandardGreeting);
    root.registerClass(LegacyGreeting);
    root.registerClass(StaticGreeting);
    root.validate();
    return [
      (await root.resolve(StandardGreeting)).text(),
      (await root.resolve(LegacyGreeting)).text(),
      (await root.resolve(StaticGreeting)).text(),
    ].join('|');
  } finally {
    await root.dispose();
  }
}

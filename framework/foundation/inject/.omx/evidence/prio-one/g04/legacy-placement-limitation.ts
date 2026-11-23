import { Container, Inject, InjectAll, InjectOptional, Injectable, Singleton, createToken } from '@coldmind/inject';
const Word = createToken<string>('legacy-type-word');
@Injectable()
abstract class Base {
  constructor(@Inject(Word) readonly word: string) {}
}
@Singleton()
class Legacy extends Base {
  @Inject(Word) field!: string;
  @InjectOptional(Word) optional?: string;
  @InjectAll(Word) set words(values: string[]) {}
  // @ts-expect-error Injection is not valid on legacy methods.
  @Inject(Word) invalidMethod() {}
  // @ts-expect-error Class registration is not valid on legacy fields.
  @Injectable() invalidField!: string;
  method(
    // @ts-expect-error Only constructor parameters can be injected.
    @Inject(Word) value: string,
  ) {}
}
const root = new Container(); root.registerClass(Legacy);
const supplied: Promise<Legacy> = root.resolve(Legacy);

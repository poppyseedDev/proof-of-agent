/**
 * Compares the IDL of the program that is deployed with the IDL in this checkout.
 *
 * The site, the runner and the SDK are deployed separately from the program, so for a
 * while clients built from the new IDL talk to the old program. That only works if the
 * new IDL changes nothing the old program reads: same instruction names and arguments,
 * and the old accounts in the old order. A program ignores extra accounts after the
 * ones it expects, so new accounts are safe at the end of the list and nowhere else.
 */

type Account = { name: string; writable?: boolean; signer?: boolean; address?: string; pda?: unknown };
type Instruction = { name: string; discriminator: number[]; accounts: Account[]; args: unknown[] };
type Named = { name: string };
export type CompatIdl = {
  address: string;
  instructions: Instruction[];
  accounts?: (Named & { discriminator: number[] })[];
  types?: Named[];
  errors?: { code: number; name: string }[];
};

export type Compat = {
  /** Clients built from the new IDL fail against the deployed program. Must be empty before deploying any client. */
  breaking: string[];
  /** Works only once the program is upgraded; harmless until then. */
  needsUpgrade: string[];
  /** After the upgrade, clients built before this change fail here. Deploy every client first, then the program. */
  clientsFirst: string[];
};

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const byName = <T extends Named>(list: T[] | undefined) => new Map((list ?? []).map((x) => [x.name, x]));

export function compareIdl(deployed: CompatIdl, next: CompatIdl): Compat {
  const out: Compat = { breaking: [], needsUpgrade: [], clientsFirst: [] };
  if (deployed.address !== next.address) out.breaking.push(`program address changed from ${deployed.address} to ${next.address}`);

  const before = byName(deployed.instructions);
  for (const ix of next.instructions) {
    const old = before.get(ix.name);
    if (!old) {
      out.needsUpgrade.push(`${ix.name} is new: the deployed program rejects it`);
      continue;
    }
    if (!same(old.discriminator, ix.discriminator)) out.breaking.push(`${ix.name}: discriminator changed`);
    if (!same(old.args, ix.args)) out.breaking.push(`${ix.name}: arguments changed`);
    old.accounts.forEach((a, i) => {
      const b = ix.accounts[i];
      if (!b) out.breaking.push(`${ix.name}: account ${i + 1} (${a.name}) was removed`);
      else if (b.name !== a.name) {
        out.breaking.push(`${ix.name}: account ${i + 1} is now ${b.name}, but the deployed program expects ${a.name} there. Add new accounts at the end.`);
      } else if (!!a.signer !== !!b.signer || !!a.writable !== !!b.writable || !same(a.address, b.address) || !same(a.pda, b.pda)) {
        out.breaking.push(`${ix.name}: account ${a.name} changed (signer, writable, address or seeds)`);
      }
    });
    const added = ix.accounts.slice(old.accounts.length).map((a) => a.name);
    if (added.length) {
      out.needsUpgrade.push(`${ix.name}: sends ${added.join(", ")} at the end, which the deployed program ignores`);
      out.clientsFirst.push(`${ix.name}: the upgraded program requires ${added.join(", ")}`);
    }
  }
  for (const ix of deployed.instructions) {
    if (!next.instructions.some((x) => x.name === ix.name)) out.clientsFirst.push(`${ix.name} is removed by the upgrade`);
  }

  // Accounts, events and arguments are decoded with these layouts.
  const types = byName(next.types);
  for (const t of deployed.types ?? []) {
    const now = types.get(t.name);
    if (!now) out.breaking.push(`type ${t.name} was removed: accounts or events of the deployed program cannot be decoded`);
    else if (!same(t, now)) out.breaking.push(`type ${t.name} changed layout: data written by the deployed program would be misread`);
  }
  const accounts = byName(next.accounts);
  for (const a of deployed.accounts ?? []) {
    const now = accounts.get(a.name);
    if (!now || !same(a.discriminator, now.discriminator)) out.breaking.push(`account ${a.name}: discriminator changed or removed`);
  }
  const errors = new Map((next.errors ?? []).map((e) => [e.code, e.name]));
  for (const e of deployed.errors ?? []) {
    if (errors.get(e.code) !== e.name) out.breaking.push(`error ${e.code} was ${e.name}: codes must stay and new ones go at the end`);
  }
  return out;
}

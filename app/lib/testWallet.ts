import {
  BaseSignerWalletAdapter,
  isVersionedTransaction,
  WalletName,
  WalletNotConnectedError,
  WalletReadyState,
} from "@solana/wallet-adapter-base";
import { Keypair, type Transaction, type VersionedTransaction } from "@solana/web3.js";

export const TestWalletName = "Test wallet" as WalletName<"Test wallet">;

/** Where the key lives. One key per cluster, so a devnet key never signs on another network. */
export const testWalletStorageKey = (cluster: string) => `poa-test-wallet:${cluster}`;

type Storage = Pick<globalThis.Storage, "getItem" | "setItem" | "removeItem">;

const browserStorage = (): Storage | null => {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null; // blocked site data or a sandboxed frame
  }
};

/** The stored keypair, or a new one saved for next time. Falls back to an unsaved key when storage is unavailable. */
export function loadOrCreateKeypair(storage: Storage | null, key: string): Keypair {
  try {
    const saved = storage?.getItem(key);
    if (saved) return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(saved)));
  } catch {
    // unreadable entry: replace it below
  }
  const kp = Keypair.generate();
  try {
    storage?.setItem(key, JSON.stringify(Array.from(kp.secretKey)));
  } catch {
    // storage full or blocked: the key lasts for this page only
  }
  return kp;
}

/**
 * A no-install wallet for trying the app on devnet or localnet. The key is kept in this
 * browser's localStorage, so it survives reloads and a trader can still cancel or claim
 * their positions later. It holds only test SOL; clearing site data loses it.
 * Never offered on mainnet (see Providers).
 */
export class TestWalletAdapter extends BaseSignerWalletAdapter {
  name = TestWalletName;
  url = "https://dev.proofofagent.dev/start";
  icon =
    "data:image/svg+xml;base64," +
    (typeof btoa === "function"
      ? btoa('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="#f5a524"/><path d="M11 9h10v3h-3.5v11h-3V12H11z" fill="#1a1a1a"/></svg>')
      : "");
  supportedTransactionVersions = new Set(["legacy", 0] as const);
  private keypair: Keypair | null = null;

  constructor(private readonly cluster: string, private readonly storage: Storage | null = browserStorage()) {
    super();
  }

  get connecting() {
    return false;
  }

  get publicKey() {
    return this.keypair?.publicKey ?? null;
  }

  get readyState() {
    return WalletReadyState.Loadable;
  }

  async connect() {
    this.keypair = loadOrCreateKeypair(this.storage, testWalletStorageKey(this.cluster));
    this.emit("connect", this.keypair.publicKey);
  }

  /** Disconnecting keeps the saved key, so connecting again returns to the same wallet and its positions. */
  async disconnect() {
    this.keypair = null;
    this.emit("disconnect");
  }

  async signTransaction<T extends Transaction | VersionedTransaction>(tx: T): Promise<T> {
    if (!this.keypair) throw new WalletNotConnectedError();
    if (isVersionedTransaction(tx)) tx.sign([this.keypair]);
    else tx.partialSign(this.keypair);
    return tx;
  }
}

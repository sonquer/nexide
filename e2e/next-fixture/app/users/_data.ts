/** Static user catalogue used by the users index and `[id]` pages. */
export interface User {
  id: string;
  name: string;
  role: string;
  avatarHue: number;
}

export const USERS: ReadonlyArray<User> = [
  { id: "1", name: "Ada Lovelace", role: "Founding engineer", avatarHue: 280 },
  { id: "2", name: "Grace Hopper", role: "Compiler whisperer", avatarHue: 200 },
  { id: "42", name: "Alan Turing", role: "Runtime architect", avatarHue: 140 },
];

/** Lookup helper that returns `undefined` when the id is unknown. */
export function findUser(id: string): User | undefined {
  return USERS.find((user) => user.id === id);
}

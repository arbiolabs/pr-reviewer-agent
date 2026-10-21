export type SignupInput = { email: string; password: string };

export function validateSignup(input: SignupInput): string[] {
  const errors: string[] = [];
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(input.email.trim())) errors.push("email is invalid");
  if (input.password.length < 12) errors.push("password must be at least 12 characters");
  return errors;
}

export function validateInvite(email: string): string[] {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email.trim()) ? [] : ["email is invalid"];
}

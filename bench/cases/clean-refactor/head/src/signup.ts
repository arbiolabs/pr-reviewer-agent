import { isValidEmail } from "./email.ts";

export type SignupInput = { email: string; password: string };

export function validateSignup(input: SignupInput): string[] {
  const errors: string[] = [];
  if (!isValidEmail(input.email)) errors.push("email is invalid");
  if (input.password.length < 12) errors.push("password must be at least 12 characters");
  return errors;
}

export function validateInvite(email: string): string[] {
  return isValidEmail(email) ? [] : ["email is invalid"];
}

import axios from "axios";
import { useAuthStore } from "@/store/auth";
import type {
  Market,
  Order,
  PlaceOrderPayload,
  Position,
  User,
  WithdrawalRequest,
  WithdrawalRequestPayload,
} from "@/lib/types";

export const API_URL =
  process.env.NEXT_PUBLIC_API_URL ?? "http://localhost:3000";

export const api = axios.create({ baseURL: API_URL });

api.interceptors.request.use((config) => {
  const token = useAuthStore.getState().token;
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  }
  return config;
});

api.interceptors.response.use(
  (response) => response,
  (error) => {
    if (error.response?.status === 401) {
      useAuthStore.getState().logout();
      if (typeof window !== "undefined") {
        // Outside the React tree (axios interceptor) - no router instance
        // available here, and a hard navigation is fine since we also want
        // to drop all in-memory query cache state on auth expiry.
        // eslint-disable-next-line @next/next/no-location-assign-relative-destination
        window.location.href = "/login";
      }
    }
    return Promise.reject(error);
  },
);

export interface AuthResponse {
  token: string;
  user: User;
}

export async function signup(username: string, password: string) {
  const res = await api.post<User>("/signup", { username, password });
  return res.data;
}

export async function signin(username: string, password: string) {
  const res = await api.post<AuthResponse>("/signin", { username, password });
  return res.data;
}

export async function getMe() {
  const res = await api.get<User>("/me");
  return res.data;
}

export async function getMarkets() {
  const res = await api.get<Market[]>("/markets");
  return res.data;
}

export async function placeOrder(payload: PlaceOrderPayload) {
  const res = await api.post<Order>("/orders", payload);
  return res.data;
}

export async function cancelOrder(orderId: number) {
  const res = await api.delete<Order>(`/orders/${orderId}`);
  return res.data;
}

export async function listOrders() {
  const res = await api.get<Order[]>("/orders");
  return res.data;
}

export async function listPositions() {
  const res = await api.get<Position[]>("/positions");
  return res.data;
}

export async function requestWithdrawal(payload: WithdrawalRequestPayload) {
  const res = await api.post<WithdrawalRequest>("/withdrawals", payload);
  return res.data;
}

export async function listWithdrawals() {
  const res = await api.get<WithdrawalRequest[]>("/withdrawals");
  return res.data;
}

function extractErrorMessage(error: unknown, fallback: string): string {
  if (axios.isAxiosError(error)) {
    const data = error.response?.data;
    if (typeof data === "string" && data.length > 0) return data;
    if (data && typeof data === "object" && "message" in data) {
      return String((data as { message: unknown }).message);
    }
  }
  return fallback;
}

export function apiErrorMessage(error: unknown, fallback = "Something went wrong") {
  return extractErrorMessage(error, fallback);
}

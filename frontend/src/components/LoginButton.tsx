import { useGoogleLogin } from "@react-oauth/google";

interface Props {
  onLogin: (token: string) => void;
}

export function LoginButton({ onLogin }: Props) {
  const login = useGoogleLogin({
    onSuccess: (response) => onLogin(response.access_token),
    onError: () => console.error("Login failed"),
  });

  return (
    <div className="flex min-h-screen items-center justify-center bg-gray-50">
      <div className="rounded-lg bg-white p-8 shadow-md text-center">
        <h1 className="mb-6 text-2xl font-semibold text-gray-800">
          Steve Assist Dashboard
        </h1>
        <button
          onClick={() => login()}
          className="rounded-md bg-blue-600 px-6 py-3 text-white font-medium hover:bg-blue-700 transition-colors cursor-pointer"
        >
          Sign in with Google
        </button>
      </div>
    </div>
  );
}

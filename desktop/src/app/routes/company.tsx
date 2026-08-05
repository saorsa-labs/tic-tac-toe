import { createFileRoute } from "@tanstack/react-router";

import { CompanyScreen } from "@/features/company/ui/CompanyScreen";

export const Route = createFileRoute("/company")({
  component: CompanyScreen,
});

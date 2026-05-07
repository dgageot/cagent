package genai

// ProviderNameForConfig maps the project's internal provider type strings
// to the GenAI semconv provider names. Unknown providers fall through
// unchanged.
func ProviderNameForConfig(internalName string) string {
	switch internalName {
	case "openai", "openai_chatcompletions", "openai_responses":
		return ProviderOpenAI
	case "anthropic":
		return ProviderAnthropic
	case "amazon-bedrock":
		return ProviderAWSBedrock
	case "google":
		return ProviderGCPGenAI
	case "vertexai", "google-vertex":
		return ProviderGCPVertexAI
	case "azure", "azure-openai":
		return ProviderAzureAI
	case "dmr":
		return ProviderDMR
	default:
		return internalName
	}
}
